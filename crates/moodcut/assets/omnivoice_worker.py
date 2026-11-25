# MoodCut's OmniVoice worker: voice cloning and speech generation with OmniVoice
# (k2-fsa/OmniVoice), run locally. MoodCut writes this file next to the Python
# environment it set up and talks to it over stdin/stdout, one JSON object per line:
#
#   {"op": "clone", "ref_audio": "ref.wav", "ref_text": "...or null", "out": "prompt.pt"}
#   {"op": "generate", "voice": "prompt.pt", "text": "...", "language": "en", "speed": 1.0,
#    "num_step": 32, "out": "line.wav"}
#   {"op": "ping"}
#
# Every reply is one line starting with "@@MC " followed by JSON. Anything else on
# stdout (library prints) is moved to stderr, which MoodCut keeps for error messages.
#
#   python omnivoice_worker.py --check                 import test, prints versions and the device
#   python omnivoice_worker.py --model DIR [--device D] serve requests

import json
import os
import sys
import time
import traceback

# Replies go to a private copy of stdout; fd 1 (and print) go to stderr from now on,
# so nothing a library prints can be mistaken for a reply.
_OUT = os.fdopen(os.dup(1), "w", encoding="utf-8", buffering=1)
os.dup2(2, 1)
sys.stdout = sys.stderr


def reply(**kw):
    _OUT.write("@@MC " + json.dumps(kw, ensure_ascii=False) + "\n")
    _OUT.flush()


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


def check():
    import torch
    import omnivoice  # noqa: F401

    version = "?"
    try:
        from importlib.metadata import version as v

        version = v("omnivoice")
    except Exception:
        pass
    dev = best_device(torch)
    gpu = torch.cuda.get_device_name(0) if dev.startswith("cuda") else None
    reply(ok=True, torch=torch.__version__, omnivoice=version, device=dev, gpu=gpu, python=sys.version.split()[0])


def voice_prompt_class():
    try:
        from omnivoice import VoiceClonePrompt
    except ImportError:
        from omnivoice.models.omnivoice import VoiceClonePrompt
    return VoiceClonePrompt


def serve(model_dir, device):
    import soundfile as sf
    import torch
    from omnivoice import OmniVoice

    VoiceClonePrompt = voice_prompt_class()
    if not device:
        device = best_device(torch)
    # half precision on GPUs; the CPU needs float32
    dtype = torch.float32 if device == "cpu" else torch.float16
    t0 = time.time()
    model = OmniVoice.from_pretrained(model_dir, device_map=device, dtype=dtype)
    sr = int(getattr(model, "sampling_rate", 24000) or 24000)
    reply(ready=True, device=device, sr=sr, load_s=round(time.time() - t0, 1))

    prompts = {}  # path -> (mtime, VoiceClonePrompt)

    def load_prompt(path):
        m = os.path.getmtime(path)
        hit = prompts.get(path)
        if hit and hit[0] == m:
            return hit[1]
        p = VoiceClonePrompt.load(path)
        prompts[path] = (m, p)
        return p

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
        try:
            t0 = time.time()
            if op == "ping":
                reply(id=rid, ok=True)
            elif op == "clone":
                prompt = model.create_voice_clone_prompt(ref_audio=req["ref_audio"], ref_text=req.get("ref_text") or None)
                prompt.save(req["out"])
                reply(id=rid, ok=True, ref_text=getattr(prompt, "ref_text", None) or "", secs=round(time.time() - t0, 1))
            elif op == "generate":
                kw = {}
                if req.get("voice"):
                    kw["voice_clone_prompt"] = load_prompt(req["voice"])
                if req.get("instruct"):
                    kw["instruct"] = req["instruct"]
                if req.get("language"):
                    kw["language"] = req["language"]
                if req.get("speed"):
                    kw["speed"] = float(req["speed"])
                if req.get("num_step"):
                    kw["num_step"] = int(req["num_step"])
                audio = model.generate(text=req["text"], **kw)
                wav = audio[0]
                sf.write(req["out"], wav, sr)
                reply(id=rid, ok=True, dur=round(len(wav) / float(sr), 3), secs=round(time.time() - t0, 1))
            else:
                reply(id=rid, ok=False, error="unknown op %r" % op)
        except Exception as e:  # keep serving: one bad line shouldn't unload the model
            traceback.print_exc()
            msg = str(e) or e.__class__.__name__
            if "out of memory" in msg.lower():
                msg += " (the GPU ran out of memory: close other GPU apps, or generate shorter paragraphs)"
            reply(id=rid, ok=False, error=msg)
            if torch.cuda.is_available():
                torch.cuda.empty_cache()


def main():
    args = sys.argv[1:]
    if "--check" in args:
        try:
            check()
        except Exception as e:
            traceback.print_exc()
            reply(ok=False, error=str(e) or e.__class__.__name__)
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
        reply(ready=False, error=str(e) or e.__class__.__name__)
        sys.exit(1)


if __name__ == "__main__":
    main()
