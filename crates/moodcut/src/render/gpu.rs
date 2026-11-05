//! The GPUs in this machine, and which one MoodCut gives video work to.
//!
//! Compositing runs on the CPU. The GPU's media engines take the work around it:
//! encoding exports (NVENC, AMF, QuickSync) and decoding heavy footage (4K, HEVC,
//! 10-bit), which on a 4K HEVC test clip cut the CPU time spent decoding by more
//! than half and leaves that CPU to the compositor. The best GPU is a discrete
//! card before integrated graphics, then the one with the most video memory.
//! `MOODCUT_GPU` picks another (`nvidia`, `amd`, `intel` or part of its name), or
//! `off` for CPU only.

use std::path::Path;
use std::process::Stdio;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

impl Vendor {
    pub fn from_pci(id: u32) -> Vendor {
        match id {
            0x10de => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            0x106b => Vendor::Apple,
            _ => Vendor::Other,
        }
    }

    /// The vendor whose media engine an FFmpeg hardware encoder runs on.
    pub fn of_encoder(enc: &str) -> Vendor {
        if enc.ends_with("_nvenc") {
            Vendor::Nvidia
        } else if enc.ends_with("_amf") {
            Vendor::Amd
        } else if enc.ends_with("_qsv") {
            Vendor::Intel
        } else if enc.ends_with("_videotoolbox") {
            Vendor::Apple
        } else {
            Vendor::Other
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Vendor::Nvidia => "nvidia",
            Vendor::Amd => "amd",
            Vendor::Intel => "intel",
            Vendor::Apple => "apple",
            Vendor::Other => "other",
        }
    }

    /// Between GPUs that rank the same otherwise: the better encoder first.
    pub fn tiebreak(self) -> u8 {
        match self {
            Vendor::Nvidia => 4,
            Vendor::Apple => 3,
            Vendor::Amd => 2,
            Vendor::Intel => 1,
            Vendor::Other => 0,
        }
    }
}

const GIB: u64 = 1 << 30;

#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    /// Dedicated video memory in bytes (0: none, as for integrated graphics, or unknown).
    pub vram: u64,
    /// Windows: the D3D11 adapter index. Linux: the DRM render node.
    pub device: Option<String>,
}

impl Gpu {
    /// A graphics card with memory of its own. Apple's media engines count too:
    /// they decode faster than the CPU.
    pub fn discrete(&self) -> bool {
        match self.vendor {
            Vendor::Nvidia | Vendor::Apple => true,
            _ => self.vram >= 2 * GIB && !amd_integrated(&self.name),
        }
    }

    pub fn label(&self) -> String {
        if !self.discrete() {
            format!("{} (integrated)", self.name)
        } else if self.vram >= GIB {
            format!("{} ({} GB)", self.name, (self.vram as f64 / GIB as f64).round())
        } else {
            self.name.clone()
        }
    }

    /// `want` is a vendor (`nvidia`, `amd`, `intel`) or part of the name (`4050`, `arc`).
    pub fn matches(&self, want: &str) -> bool {
        let w = want.trim().to_ascii_lowercase();
        !w.is_empty() && (self.vendor.word() == w || self.name.to_ascii_lowercase().contains(&w))
    }
}

/// AMD's integrated graphics are named "AMD Radeon(TM) Graphics", "Radeon 780M
/// Graphics", "Radeon Vega 8 Graphics"; its cards are "Radeon RX 7800 XT" and the like.
fn amd_integrated(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n.contains("radeon") && n.ends_with("graphics")
}

/// What `MOODCUT_GPU` asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Pref {
    Auto,
    Off,
    Named(String),
}

impl Pref {
    pub fn from_env() -> Pref {
        Pref::parse(&std::env::var("MOODCUT_GPU").unwrap_or_default())
    }

    pub fn parse(s: &str) -> Pref {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "best" => Pref::Auto,
            "off" | "none" | "cpu" | "0" | "false" => Pref::Off,
            other => Pref::Named(other.to_string()),
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Pref::Named(n) => Some(n),
            _ => None,
        }
    }
}

/// Best first: the one `MOODCUT_GPU` names, then discrete before integrated, then
/// more video memory, then the better encoder.
pub fn rank(gpus: &mut [Gpu], prefer: Option<&str>) {
    gpus.sort_by_key(|g| std::cmp::Reverse((prefer.is_some_and(|p| g.matches(p)), g.discrete(), g.vram, g.vendor.tiebreak())));
}

/// When video is decoded on the GPU (`MOODCUT_HWDEC`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwDec {
    /// CPU only.
    Off,
    /// Heavy footage (4K, HEVC, 10-bit...) when the best GPU is a discrete one (the default).
    Auto,
    /// Every video, on any GPU.
    All,
}

impl HwDec {
    pub fn from_env() -> HwDec {
        HwDec::parse(&std::env::var("MOODCUT_HWDEC").unwrap_or_default())
    }

    pub fn parse(s: &str) -> HwDec {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "all" | "true" => HwDec::All,
            "0" | "off" | "false" | "none" => HwDec::Off,
            _ => HwDec::Auto,
        }
    }
}

/// The GPUs in this machine (unranked). Uses FFmpeg's own view on Windows, so
/// adapter numbers are the ones its decoders take.
pub fn detect(ffmpeg: &Path) -> Vec<Gpu> {
    if cfg!(windows) {
        windows(ffmpeg)
    } else if cfg!(target_os = "macos") {
        vec![Gpu { name: "Apple GPU".into(), vendor: Vendor::Apple, vram: 0, device: None }]
    } else {
        linux()
    }
}

fn windows(ffmpeg: &Path) -> Vec<Gpu> {
    let reg = reg_adapters();
    let mut out = vec![];
    for i in 0..8u32 {
        let Some((ven, name, ok)) = d3d11_adapter(ffmpeg, i) else { break };
        // 1414 is Microsoft's software renderer
        if !ok || ven == 0x1414 {
            continue;
        }
        let vram = reg.iter().find(|a| a.name == name).map(|a| a.vram()).unwrap_or(0);
        out.push(Gpu { name, vendor: Vendor::from_pci(ven), vram, device: Some(i.to_string()) });
    }
    if out.is_empty() {
        // an FFmpeg that doesn't log its adapters: the registry alone (no adapter numbers)
        for a in reg {
            if let Some(ven) = a.vendor.filter(|v| *v != 0x1414) {
                let vram = a.vram();
                out.push(Gpu { name: a.name, vendor: Vendor::from_pci(ven), vram, device: None });
            }
        }
    }
    out
}

/// Asks FFmpeg to open D3D11 adapter `i`: (PCI vendor, name, opened), None past the last one.
fn d3d11_adapter(ffmpeg: &Path, i: u32) -> Option<(u32, String, bool)> {
    let out = super::ffmpeg::command(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "verbose", "-init_hw_device", &format!("d3d11va=gpu:{i}")])
        .args(["-f", "lavfi", "-i", "nullsrc=s=64x64:d=0.04", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    parse_d3d11(&String::from_utf8_lossy(&out.stderr))
}

/// "[D3D11VA @ ...] Using device 10de:28a1 (NVIDIA GeForce RTX 4050 Laptop GPU)."
fn parse_d3d11(log: &str) -> Option<(u32, String, bool)> {
    let line = log.lines().find(|l| l.contains("Using device "))?;
    let rest = &line[line.find("Using device ")? + "Using device ".len()..];
    let (ids, name) = rest.split_once(' ')?;
    let ven = u32::from_str_radix(ids.split(':').next()?, 16).ok()?;
    let name = name.trim().trim_end_matches('.');
    let name = name.strip_prefix('(').and_then(|n| n.strip_suffix(')')).unwrap_or(name).to_string();
    Some((ven, name, !log.contains("Failed to create")))
}

#[derive(Debug, Default, PartialEq)]
struct RegAdapter {
    key: String,
    name: String,
    vendor: Option<u32>,
    qw_memory: Option<u64>,
    memory: Option<u64>,
}

impl RegAdapter {
    fn vram(&self) -> u64 {
        self.qw_memory.or(self.memory).unwrap_or(0)
    }
}

/// Display adapters as the driver registers them: name, vendor, video memory.
fn reg_adapters() -> Vec<RegAdapter> {
    let reg = std::env::var("SystemRoot").map(|r| Path::new(&r).join("System32").join("reg.exe")).unwrap_or_else(|_| "reg".into());
    let class = r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let out = super::ffmpeg::command(&reg).args(["query", class, "/s"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output();
    out.map(|o| parse_reg(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

fn parse_reg(s: &str) -> Vec<RegAdapter> {
    let mut v: Vec<RegAdapter> = vec![];
    let mut cur: Option<usize> = None;
    for line in s.lines() {
        if line.starts_with("HKEY_") {
            // each adapter is a numbered key right under the class: ...\{4d36e968-...}\0000
            let id = line.trim().split('\\').skip_while(|p| !p.starts_with('{')).nth(1).unwrap_or("");
            cur = (id.len() == 4 && id.bytes().all(|b| b.is_ascii_digit())).then(|| match v.iter().position(|a| a.key == id) {
                Some(i) => i,
                None => {
                    v.push(RegAdapter { key: id.to_string(), ..Default::default() });
                    v.len() - 1
                }
            });
            continue;
        }
        let Some(i) = cur else { continue };
        // "    DriverDesc    REG_SZ    NVIDIA GeForce RTX 4050 Laptop GPU"
        let mut f = line.trim().splitn(3, "    ");
        let (Some(name), Some(ty)) = (f.next(), f.next()) else { continue };
        let data = f.next().unwrap_or("").trim();
        match name {
            "DriverDesc" => v[i].name = data.to_string(),
            "MatchingDeviceId" => v[i].vendor = pci_vendor(data),
            "HardwareInformation.qwMemorySize" => v[i].qw_memory = reg_number(ty, data),
            "HardwareInformation.MemorySize" => v[i].memory = reg_number(ty, data),
            _ => {}
        }
    }
    v.retain(|a| !a.name.is_empty());
    v
}

/// "pci\ven_10de&dev_28a1&subsys_..." -> 0x10de
fn pci_vendor(id: &str) -> Option<u32> {
    let l = id.to_ascii_lowercase();
    let i = l.find("ven_")?;
    u32::from_str_radix(l.get(i + 4..i + 8)?, 16).ok()
}

fn reg_number(ty: &str, data: &str) -> Option<u64> {
    match ty {
        "REG_QWORD" | "REG_DWORD" => u64::from_str_radix(data.trim_start_matches("0x"), 16).ok(),
        // little-endian bytes in hex: "00000080" is 0x80000000
        "REG_BINARY" => {
            if data.is_empty() || data.len() > 16 || data.len() % 2 != 0 {
                return None;
            }
            let bytes: Option<Vec<u8>> = (0..data.len() / 2).map(|k| u8::from_str_radix(&data[2 * k..2 * k + 2], 16).ok()).collect();
            Some(bytes?.iter().rev().fold(0u64, |a, b| (a << 8) | *b as u64))
        }
        _ => None,
    }
}

fn linux() -> Vec<Gpu> {
    let root = Path::new("/sys/class/drm");
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut nodes: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("renderD")).collect();
    nodes.sort();
    nodes
        .into_iter()
        .map(|n| {
            let dev = root.join(&n).join("device");
            let read = |f: &str| std::fs::read_to_string(dev.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
            let vendor = Vendor::from_pci(u32::from_str_radix(read("vendor").trim_start_matches("0x"), 16).unwrap_or(0));
            // amdgpu reports its memory; integrated parts report their small carve-out
            let vram = read("mem_info_vram_total").parse::<u64>().unwrap_or(0);
            let brand = match vendor {
                Vendor::Nvidia => "NVIDIA",
                Vendor::Amd => "AMD",
                Vendor::Intel => "Intel",
                _ => "Other",
            };
            Gpu { name: format!("{brand} GPU ({n})"), vendor, vram, device: Some(format!("/dev/dri/{n}")) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(name: &str, vendor: Vendor, vram_gb: u64) -> Gpu {
        Gpu { name: name.into(), vendor, vram: vram_gb * GIB, device: None }
    }

    #[test]
    fn reads_ffmpeg_adapter_lines() {
        let nv = "[D3D11VA @ 000002341db15a00] Selecting d3d11va adapter 0\n[D3D11VA @ 000002341db15a00] Using device 10de:28a1 (NVIDIA GeForce RTX 4050 Laptop GPU).\r\n";
        assert_eq!(parse_d3d11(nv), Some((0x10de, "NVIDIA GeForce RTX 4050 Laptop GPU".into(), true)));
        let intel = "[D3D11VA @ 0] Using device 8086:468b (Intel(R) UHD Graphics).\n";
        assert_eq!(parse_d3d11(intel), Some((0x8086, "Intel(R) UHD Graphics".into(), true)));
        let soft = "[D3D11VA @ 0] Using device 1414:008c (Microsoft Basic Render Driver).\n[D3D11VA @ 0] Failed to create Direct3D device (887a0004)\n";
        assert_eq!(parse_d3d11(soft).map(|x| x.2), Some(false));
        assert_eq!(parse_d3d11("[D3D11VA @ 0] Selecting d3d11va adapter 3\n"), None, "past the last adapter");
    }

    #[test]
    fn reads_registry_adapters() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\r\n    Class    REG_SZ    Display\r\n\r\n\
HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0000\r\n    DriverDesc    REG_SZ    NVIDIA GeForce RTX 4050 Laptop GPU\r\n    MatchingDeviceId    REG_SZ    pci\\ven_10de&dev_28a1&subsys_0b0c1028\r\n    HardwareInformation.MemorySize    REG_BINARY    00000080\r\n    HardwareInformation.qwMemorySize    REG_QWORD    0x17fd00000\r\n\r\n\
HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0000\\Settings\r\n    Device Description    REG_SZ    whatever\r\n\r\n\
HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0001\r\n    DriverDesc    REG_SZ    Intel(R) UHD Graphics\r\n    MatchingDeviceId    REG_SZ    PCI\\VEN_8086&DEV_468B\r\n\r\n\
HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\Properties\r\n";
        let v = parse_reg(out);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].name.as_str(), v[0].vendor, v[0].vram()), ("NVIDIA GeForce RTX 4050 Laptop GPU", Some(0x10de), 0x17fd00000));
        assert_eq!(v[0].memory, Some(0x8000_0000), "REG_BINARY is little-endian");
        assert_eq!((v[1].name.as_str(), v[1].vendor, v[1].vram()), ("Intel(R) UHD Graphics", Some(0x8086), 0));
    }

    #[test]
    fn discrete_cards_rank_first() {
        let mut v = vec![gpu("Intel(R) UHD Graphics", Vendor::Intel, 0), gpu("NVIDIA GeForce RTX 4050 Laptop GPU", Vendor::Nvidia, 6)];
        rank(&mut v, None);
        assert_eq!(v[0].vendor, Vendor::Nvidia);
        assert!(!v[1].discrete());
        // an AMD laptop chip's 4 GB carve-out doesn't make it a card; an Intel Arc card is one
        let mut v = vec![gpu("AMD Radeon(TM) Graphics", Vendor::Amd, 4), gpu("Intel(R) Arc(TM) A770 Graphics", Vendor::Intel, 16)];
        rank(&mut v, None);
        assert_eq!(v[0].vendor, Vendor::Intel);
        assert!(!v[1].discrete());
        // the bigger card wins; MOODCUT_GPU overrides
        let mut v = vec![gpu("AMD Radeon RX 7600", Vendor::Amd, 8), gpu("AMD Radeon RX 7900 XTX", Vendor::Amd, 24), gpu("Intel(R) UHD Graphics", Vendor::Intel, 0)];
        rank(&mut v, None);
        assert_eq!(v[0].name, "AMD Radeon RX 7900 XTX");
        rank(&mut v, Some("intel"));
        assert_eq!(v[0].vendor, Vendor::Intel);
        rank(&mut v, Some("7600"));
        assert_eq!(v[0].name, "AMD Radeon RX 7600");
    }

    #[test]
    fn settings() {
        assert_eq!(Pref::parse(""), Pref::Auto);
        assert_eq!(Pref::parse("OFF"), Pref::Off);
        assert_eq!(Pref::parse(" NVIDIA "), Pref::Named("nvidia".into()));
        assert_eq!(HwDec::parse(""), HwDec::Auto);
        assert_eq!(HwDec::parse("1"), HwDec::All);
        assert_eq!(HwDec::parse("off"), HwDec::Off);
        assert_eq!(Vendor::of_encoder("hevc_nvenc"), Vendor::Nvidia);
        assert_eq!(Vendor::of_encoder("h264_qsv"), Vendor::Intel);
        assert_eq!(reg_number("REG_DWORD", "0x10"), Some(16));
        assert_eq!(pci_vendor("PCI\\VEN_1002&DEV_73BF"), Some(0x1002));
    }
}
