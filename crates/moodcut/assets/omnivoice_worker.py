# MoodCut's OmniVoice worker: voice cloning and speech generation with OmniVoice
# (k2-fsa/OmniVoice), run locally. MoodCut writes this file next to the Python
# environment it set up and talks to it over stdin/stdout, one JSON object per line:
#
#   {"id": 1, "op": "clone", "ref_audio": "ref.wav", "ref_text": "...or null", "out": "prompt.pt",
#    "auto_window": true, "ref_out": "ref-used.wav"}
#   {"id": 2, "op": "generate", "voice": "prompt.pt", "text": "...", "language": "English",
#    "speed": 1.0, "num_step": 32, "seed": 7, "numbers": true, "loudness": -19, "out": "line.wav"}
#   {"id": 3, "op": "info"}
#
# Every message to MoodCut is one line starting with "@@MC " followed by JSON: a reply
# ({"id", "ok", ...} or {"id", "ok": false, "error", "etype", "trace"}) or progress while
# a request runs ({"id", "progress": 0..1, "stage": "..."}). Anything else on stdout (library
# prints) is moved to stderr, which MoodCut keeps for error details.
#
#   python omnivoice_worker.py --check                  import test, prints versions and the device
#   python omnivoice_worker.py --model DIR [--device D] serve requests
#   python omnivoice_worker.py --selftest               pure-numpy checks (no model needed)

import json
import os
import re
import sys
import threading
import time
import traceback

# Replies go to a private copy of stdout; fd 1 (and print) go to stderr from now on,
# so nothing a library prints can be mistaken for a reply.
_OUT = os.fdopen(os.dup(1), "w", encoding="utf-8", buffering=1)
os.dup2(2, 1)
sys.stdout = sys.stderr
_OUT_LOCK = threading.Lock()


def reply(**kw):
    line = "@@MC " + json.dumps(kw, ensure_ascii=False) + "\n"
    with _OUT_LOCK:
        _OUT.write(line)
        _OUT.flush()


class Progress:
    """Throttled progress messages for one request."""

    def __init__(self, rid):
        self.rid = rid
        self.last = 0.0
        self.stage = ""

    def __call__(self, frac, stage=None, force=False, **extra):
        now = time.time()
        if stage and stage != self.stage:
            self.stage = stage
            force = True
        if force or now - self.last >= 0.25:
            self.last = now
            reply(id=self.rid, progress=round(max(0.0, min(1.0, frac)), 4), stage=self.stage, **extra)


def best_device(torch):
    if torch.cuda.is_available():
        return "cuda:0"
    xpu = getattr(torch, "xpu", None)
    if xpu is not None and xpu.is_available():
        return "xpu"
    mps = getattr(torch.backends, "mps", None)
    if mps is not None and mps.is_available():
        return "mps"
    return "cpu"


def pkg_version(name):
    try:
        from importlib.metadata import version as v

        return v(name)
    except Exception:
        return None


def check():
    import torch
    import omnivoice  # noqa: F401

    dev = best_device(torch)
    gpu = torch.cuda.get_device_name(0) if dev.startswith("cuda") else None
    vram = None
    if dev.startswith("cuda"):
        vram = round(torch.cuda.get_device_properties(0).total_memory / 2**30, 1)
    reply(ok=True, torch=torch.__version__, omnivoice=pkg_version("omnivoice") or "?", device=dev, gpu=gpu, vram_gb=vram, python=sys.version.split()[0], num2words=pkg_version("num2words") is not None)


# ---------------------------------------------------------------- audio helpers (numpy only)


def to_mono(x):
    import numpy as np

    x = np.asarray(x, dtype=np.float32)
    if x.ndim == 2:
        x = x.mean(axis=1) if x.shape[1] <= 8 else x.mean(axis=0)
    return x


def speech_segments(x, sr, frame=0.02, hop=0.01):
    """Speech regions [(start_s, end_s), ...] from frame energy: a noise floor from the quiet
    frames, speech well above it, short gaps bridged and clicks dropped."""
    import numpy as np

    n = int(frame * sr)
    h = int(hop * sr)
    if len(x) < n:
        return []
    count = 1 + (len(x) - n) // h
    idx = np.arange(n)[None, :] + h * np.arange(count)[:, None]
    rms = np.sqrt(np.mean(x[idx] ** 2, axis=1) + 1e-12)
    db = 20 * np.log10(rms + 1e-9)
    # the floor ignores digital silence (generated pauses, padding), which would drag it far
    # below the room noise; speech is well above the floor and within 30 dB of the loud parts
    live = db[db > -85.0]
    floor = np.percentile(live, 10) if live.size else -90.0
    top = np.percentile(db, 95)
    thresh = max(floor + 10.0, top - 30.0, -62.0)
    on = db > thresh
    # bridge gaps shorter than 150 ms (between syllables), drop blips under 80 ms
    segs = []
    start = None
    for i, v in enumerate(on):
        if v and start is None:
            start = i
        elif not v and start is not None:
            segs.append([start, i])
            start = None
    if start is not None:
        segs.append([start, len(on)])
    merged = []
    for s, e in segs:
        if merged and (s - merged[-1][1]) * hop < 0.15:
            merged[-1][1] = e
        else:
            merged.append([s, e])
    out = [(s * hop, e * hop + frame) for s, e in merged if (e - s) * hop >= 0.08]
    return out


def pick_window(segs, total, lo=3.0, hi=12.0, ideal=9.0):
    """The best reference window: starts where speech starts, ends at a pause, 3-12 s long,
    as much speech as possible (close to the ideal length) and no long silences inside."""
    if not segs:
        return None
    best = None
    for i in range(len(segs)):
        speech = 0.0
        longest_gap = 0.0
        for j in range(i, len(segs)):
            speech += segs[j][1] - segs[j][0]
            if j > i:
                longest_gap = max(longest_gap, segs[j][0] - segs[j - 1][1])
            dur = segs[j][1] - segs[i][0]
            if dur > hi:
                break
            if dur < lo:
                continue
            score = speech - 0.6 * abs(dur - ideal) * (dur > ideal) - 0.15 * abs(dur - ideal) * (dur < ideal) - 1.5 * max(0.0, longest_gap - 0.8)
            # a cut that ends at a real pause (or the end) keeps words whole
            nxt_gap = (segs[j + 1][0] - segs[j][1]) if j + 1 < len(segs) else 1.0
            if nxt_gap < 0.2:
                score -= 1.0
            if best is None or score > best[0]:
                best = (score, segs[i][0], segs[j][1], speech)
    if best is None:
        # under 3 s of speech in total: use all of it
        return (max(0.0, segs[0][0] - 0.25), min(total, segs[-1][1] + 0.3), sum(e - s for s, e in segs))
    _, s, e, speech = best
    # generous edges: soft onsets stay in, and OmniVoice trims extra silence itself
    return (max(0.0, s - 0.25), min(total, e + 0.3), speech)


def normalize_loudness(y, sr, target_db=-19.0, peak=0.89):
    """Speech at a steady level: the RMS of the voiced parts brought to target_db (about -16
    LUFS for speech), the peak kept under -1 dBFS."""
    import numpy as np

    y = np.asarray(y, dtype=np.float32)
    if y.size == 0:
        return y
    segs = speech_segments(y, sr)
    if segs:
        parts = [y[int(s * sr) : int(e * sr)] for s, e in segs]
        voiced = np.concatenate([p for p in parts if p.size]) if any(p.size for p in parts) else y
    else:
        voiced = y
    rms = float(np.sqrt(np.mean(voiced**2) + 1e-12))
    if rms < 1e-5:
        return y
    gain = 10 ** (target_db / 20.0) / rms
    pk = float(np.max(np.abs(y))) * gain
    if pk > peak:
        gain *= peak / pk
    return (y * gain).astype(np.float32)


# ---------------------------------------------------------------- text helpers

_N2W = {"english": "en", "spanish": "es", "french": "fr", "german": "de", "italian": "it", "portuguese": "pt", "dutch": "nl", "polish": "pl", "russian": "ru", "turkish": "tr", "arabic": "ar", "indonesian": "id", "vietnamese": "vi", "korean": "ko", "japanese": "ja", "thai": "th", "hindi": "hi", "bengali": "bn", "urdu": "ur"}
_NUM = re.compile(r"(?<![\w.])(\d{1,3}(?:,\d{3})+|\d+)(\.\d+)?(%?)(?![\w])")


def guess_lang(text, language):
    if language and language.lower() not in ("auto", ""):
        return _N2W.get(language.lower(), language.lower()[:2])
    latin = sum(1 for c in text if c.isalpha() and ord(c) < 0x250)
    letters = sum(1 for c in text if c.isalpha())
    return "en" if letters and latin / letters > 0.8 else None


def spell_numbers(text, language=None):
    """Digits read as words (num2words), so "2026", "3.5" and "40%" are spoken naturally.
    Inline control tags like [laughter] and pronunciations like [B EY1 S] are left alone."""
    try:
        from num2words import num2words
    except ImportError:
        return text
    lang = guess_lang(text, language)
    if not lang:
        return text

    def one(m):
        whole, frac, pct = m.group(1), m.group(2) or "", m.group(3)
        raw = whole.replace(",", "")
        try:
            n = int(raw)
            if frac:
                words = num2words(float(raw + frac), lang=lang)
            elif not pct and len(raw) == 4 and 1100 <= n <= 2099 and "," not in whole:
                words = num2words(n, lang=lang, to="year")
            else:
                words = num2words(n, lang=lang)
        except Exception:
            return m.group(0)
        if pct:
            words += " percent" if lang == "en" else " %"
        return words

    parts = re.split(r"(\[[^\]]*\])", text)
    return "".join(p if p.startswith("[") else _NUM.sub(one, p) for p in parts)


# ---------------------------------------------------------------- the model


def serve(model_dir, device):
    import numpy as np
    import soundfile as sf
    import torch
    import omnivoice.models.omnivoice as ov_mod
    from omnivoice import OmniVoice

    try:
        from omnivoice import VoiceClonePrompt
    except ImportError:
        VoiceClonePrompt = ov_mod.VoiceClonePrompt

    if not device:
        device = best_device(torch)
    on_gpu = device != "cpu"
    # Be gentle with the computer: leave two CPU cores for the editor and Windows, and keep
    # PyTorch inside the card's own memory. Past that cap it stops with "out of memory" right
    # away instead of spilling into shared system memory, which thrashes RAM and disk for minutes.
    try:
        torch.set_num_threads(max(1, (os.cpu_count() or 4) - 2))
    except Exception:
        pass
    if device.startswith("cuda") and torch.cuda.is_available():
        try:
            torch.cuda.set_per_process_memory_fraction(0.92, torch.device(device).index or 0)
        except Exception:
            pass
    # half precision on GPUs; the CPU needs float32
    dtype = torch.float32 if device == "cpu" else torch.float16
    t0 = time.time()
    reply(stage="loading the OmniVoice model", progress=0.0)
    model = OmniVoice.from_pretrained(model_dir, device_map=device, dtype=dtype)
    # inference only: no gradients, so no activations are kept for a backward pass
    model.eval()
    try:
        model.requires_grad_(False)
    except Exception:
        pass
    torch.set_grad_enabled(False)
    sr = int(getattr(model, "sampling_rate", 24000) or 24000)

    def free():
        if on_gpu and torch.cuda.is_available():
            torch.cuda.empty_cache()

    def vram():
        if device.startswith("cuda") and torch.cuda.is_available():
            free_b, total_b = torch.cuda.mem_get_info()
            return {"vram_used_gb": round((total_b - free_b) / 2**30, 2), "vram_total_gb": round(total_b / 2**30, 2), "peak_gb": round(torch.cuda.max_memory_allocated() / 2**30, 2)}
        return {}

    def run(fn):
        """Runs fn without autograd; after running out of GPU memory, frees the cache and tries once more."""
        try:
            with torch.inference_mode():
                return fn()
        except Exception as e:
            if "out of memory" not in str(e).lower():
                raise
            free()
            with torch.inference_mode():
                return fn()
        finally:
            free()

    # progress inside a generation: one forward pass per decoding step, per text chunk
    prog = {"cb": None, "chunks": 1, "chunk": -1, "steps": 32, "step": 0}
    orig_chunker = ov_mod.chunk_text_punctuation

    def chunker(*a, **k):
        out = orig_chunker(*a, **k)
        prog["chunks"] = max(1, len(out))
        return out

    ov_mod.chunk_text_punctuation = chunker
    orig_iter = model._generate_iterative

    def iterative(task, cfg):
        prog["chunk"] += 1
        prog["step"] = 0
        prog["steps"] = max(1, int(getattr(cfg, "num_step", 32)))
        return orig_iter(task, cfg)

    model._generate_iterative = iterative

    def on_forward(_m, _i, _o):
        cb = prog["cb"]
        if cb is None:
            return
        prog["step"] += 1
        done = (max(0, prog["chunk"]) + min(1.0, prog["step"] / prog["steps"])) / prog["chunks"]
        part = f" (part {prog['chunk'] + 1} of {prog['chunks']})" if prog["chunks"] > 1 else ""
        cb(0.05 + 0.88 * min(1.0, done), "speaking" + part)

    model.register_forward_hook(on_forward)

    reply(ready=True, device=device, sr=sr, load_s=round(time.time() - t0, 1), **vram())

    prompts = {}  # path -> (mtime, VoiceClonePrompt)

    def load_prompt(path):
        m = os.path.getmtime(path)
        hit = prompts.get(path)
        if hit and hit[0] == m:
            return hit[1]
        p = VoiceClonePrompt.load(path)
        prompts[path] = (m, p)
        return p

    def whisper_cached():
        hub = os.path.join(os.environ.get("HF_HOME", os.path.expanduser("~/.cache/huggingface")), "hub", "models--openai--whisper-large-v3-turbo", "snapshots")
        if not os.path.isdir(hub):
            return False
        for d in os.listdir(hub):
            if os.path.exists(os.path.join(hub, d, "model.safetensors")):
                return True
        return False

    def transcribe(wav, p):
        """Whisper on the GPU when there is room next to OmniVoice, else on the CPU; released after."""
        asr_dev = "cpu"
        if device.startswith("cuda") and torch.cuda.is_available():
            free_b, _ = torch.cuda.mem_get_info()
            if free_b > 2.2 * 2**30:
                asr_dev = device
        stop = threading.Event()
        if not whisper_cached():
            hub = os.path.join(os.environ.get("HF_HOME", os.path.expanduser("~/.cache/huggingface")), "hub", "models--openai--whisper-large-v3-turbo")

            def watch():
                total = 1.62e9
                while not stop.wait(1.0):
                    size = 0
                    for root, _dirs, files in os.walk(hub):
                        for f in files:
                            try:
                                size += os.path.getsize(os.path.join(root, f))
                            except OSError:
                                pass
                    p(0.1 + 0.4 * min(1.0, size / total), "downloading Whisper for transcription (once, 1.6 GB)", force=True, bytes=size)

            threading.Thread(target=watch, daemon=True).start()
        try:
            p(0.1, "loading Whisper", force=True)
            model.load_asr_model(device=asr_dev)
            stop.set()
            p(0.5, "transcribing the reference", force=True)
            text = run(lambda: model.transcribe((wav[None, :], sr)))
        finally:
            stop.set()
            model._asr_pipe = None
            import gc

            gc.collect()
            free()
        return text, asr_dev

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError:
            reply(ok=False, error="bad request")
            continue
        op = req.get("op")
        rid = req.get("id")
        p = Progress(rid)
        try:
            t0 = time.time()
            if op == "ping":
                reply(id=rid, ok=True)
            elif op == "info":
                reply(id=rid, ok=True, device=device, sr=sr, **vram())
            elif op == "clone":
                p(0.02, "reading the reference", force=True)
                data, file_sr = sf.read(req["ref_audio"], dtype="float32", always_2d=False)
                wav = to_mono(data)
                if file_sr != sr:
                    import torchaudio

                    wav = torchaudio.functional.resample(torch.from_numpy(wav), file_sr, sr).numpy()
                total = len(wav) / sr
                segs = speech_segments(wav, sr)
                speech_total = sum(e - s for s, e in segs)
                if speech_total < 1.0:
                    raise ValueError("NO_SPEECH: the reference has no clear speech (%.1f s found)" % speech_total)
                used = [0.0, round(total, 3)]
                if req.get("auto_window"):
                    s, e, _speech = pick_window(segs, total)
                    wav = wav[int(s * sr) : int(e * sr)]
                    used = [round(s, 3), round(e, 3)]
                if req.get("ref_out"):
                    sf.write(req["ref_out"], wav, sr)
                ref_text = (req.get("ref_text") or "").strip() or None
                asr_dev = None
                if ref_text is None:
                    ref_text, asr_dev = transcribe(wav, p)
                    if not ref_text.strip():
                        raise ValueError("NO_SPEECH: Whisper heard no words in the reference")
                p(0.85, "encoding the voice", force=True)
                prompt = run(lambda: model.create_voice_clone_prompt(ref_audio=(torch.from_numpy(wav), sr), ref_text=ref_text))
                prompt.save(req["out"])
                reply(id=rid, ok=True, ref_text=getattr(prompt, "ref_text", None) or ref_text, used=used, speech_s=round(speech_total, 2), ref_s=round(len(wav) / sr, 2), asr=asr_dev, secs=round(time.time() - t0, 1), **vram())
            elif op == "generate":
                kw = {}
                if req.get("voice"):
                    kw["voice_clone_prompt"] = load_prompt(req["voice"])
                if req.get("instruct"):
                    kw["instruct"] = req["instruct"]
                lang = req.get("language") or None
                if lang and lang.lower() != "auto":
                    kw["language"] = lang
                if req.get("speed"):
                    kw["speed"] = float(req["speed"])
                if req.get("num_step"):
                    kw["num_step"] = int(req["num_step"])
                text = req["text"]
                if req.get("numbers", True):
                    text = spell_numbers(text, lang)
                seed = req.get("seed")
                if seed is not None:
                    torch.manual_seed(int(seed))
                    np.random.seed(int(seed) % (2**32))
                p(0.02, "preparing", force=True)
                prog.update(cb=p, chunks=1, chunk=-1, step=0)
                try:
                    audio = run(lambda: model.generate(text=text, **kw))
                finally:
                    prog["cb"] = None
                p(0.95, "finishing", force=True)
                wav = audio[0]
                if req.get("loudness") is not None:
                    wav = normalize_loudness(wav, sr, float(req["loudness"]))
                sf.write(req["out"], wav, sr)
                reply(id=rid, ok=True, dur=round(len(wav) / float(sr), 3), secs=round(time.time() - t0, 1), text=text, seed=seed, **vram())
            else:
                reply(id=rid, ok=False, error="unknown op %r" % op)
        except Exception as e:  # keep serving: one bad line shouldn't unload the model
            traceback.print_exc()
            msg = str(e) or e.__class__.__name__
            tb = traceback.format_exc().strip().splitlines()[-6:]
            reply(id=rid, ok=False, error=msg, etype=e.__class__.__name__, trace="\n".join(tb))
            free()


def selftest():
    """Checks the numpy helpers without the model: run by MoodCut's setup check."""
    import numpy as np

    sr = 24000
    rng = np.random.default_rng(1)

    def tone(sec, amp=0.3):
        t = np.arange(int(sec * sr)) / sr
        return (amp * np.sin(2 * np.pi * 180 * t) * (0.6 + 0.4 * np.sin(2 * np.pi * 3 * t))).astype(np.float32)

    def hush(sec):
        return (rng.standard_normal(int(sec * sr)) * 0.002).astype(np.float32)

    x = np.concatenate([hush(1.5), tone(2.0), hush(0.4), tone(3.0), hush(0.5), tone(2.5), hush(2.0), tone(4.0), hush(1.0)])
    segs = speech_segments(x, sr)
    assert len(segs) == 4, segs
    assert abs(segs[0][0] - 1.5) < 0.05, segs
    s, e, speech = pick_window(segs, len(x) / sr)
    assert 1.1 <= s <= 1.5 and e <= len(x) / sr and 3.0 <= e - s <= 12.6, (s, e)
    y = normalize_loudness(x * 0.05, sr, -19.0)
    voiced = np.concatenate([y[int(a * sr) : int(b * sr)] for a, b in speech_segments(y, sr)])
    level = 20 * np.log10(np.sqrt(np.mean(voiced**2)))
    assert abs(level + 19.0) < 1.0, level
    assert np.max(np.abs(y)) <= 0.891
    assert spell_numbers("[laughter] 3 apples", "English") in ("[laughter] three apples", "[laughter] 3 apples")
    reply(ok=True, selftest="passed", segments=len(segs), window=[round(s, 2), round(e, 2)])


def main():
    args = sys.argv[1:]
    if "--selftest" in args:
        try:
            selftest()
        except Exception as e:
            traceback.print_exc()
            reply(ok=False, error="selftest failed: %s" % e)
            sys.exit(1)
        return
    if "--check" in args:
        try:
            check()
        except Exception as e:
            traceback.print_exc()
            reply(ok=False, error=str(e) or e.__class__.__name__, etype=e.__class__.__name__)
            sys.exit(1)
        return
    model_dir = None
    device = None
    i = 0
    while i < len(args):
        if args[i] == "--model" and i + 1 < len(args):
            model_dir = args[i + 1]
            i += 1
        elif args[i] == "--device" and i + 1 < len(args):
            device = args[i + 1]
            i += 1
        i += 1
    if not model_dir:
        reply(ready=False, error="--model is required")
        sys.exit(2)
    try:
        serve(model_dir, device)
    except Exception as e:
        traceback.print_exc()
        reply(ready=False, error=str(e) or e.__class__.__name__, etype=e.__class__.__name__, trace="\n".join(traceback.format_exc().strip().splitlines()[-6:]))
        sys.exit(1)


if __name__ == "__main__":
    main()
