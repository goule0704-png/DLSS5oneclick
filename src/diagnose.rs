//! Read the logs a session leaves behind and say why neural rendering is or is
//! not running. Answers the commonest report ("I enabled it, nothing changed")
//! without a round trip: everything needed is already in `ReShade.log` and,
//! on the Feeder path, `dlss5-feed.log` next to the game exe.

use crate::game::{self, GameStatus};
use crate::lang;
use anyhow::Result;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    pub text: String,
}

/// `NVSDK_NGX_*_Init -> 0xBAD00001` in a log: NGX itself refused. Add what the
/// system says about NGX Core, which is the usual cause on capable hardware.
/// `exe` is the game (and, for a 32-bit game, its helper) whose Windows GPU
/// preference is worth naming: on a hybrid machine a process started on the
/// iGPU gets exactly this error, because NGX does not exist there (#25).
fn ngx_init_failure_for(log: &str, exe: Option<&std::path::Path>, out: &mut Vec<Finding>) {
    let Some(line) = log
        .lines()
        .find(|l| l.contains("NVSDK_NGX") && l.contains("Init") && l.contains("0xBAD00001"))
    else {
        return;
    };
    if crate::gpupref::hybrid() {
        let set = exe.is_some_and(|e| {
            crate::gpupref::get(e).is_some_and(|v| crate::gpupref::is_high_performance(&v))
        });
        let names = crate::gpupref::real_adapters();
        out.push(bad(crate::trfmt!("More than one GPU vendor on this machine ({}), and Windows decides which one              a process starts on. Started on the integrated GPU, NGX does not exist and              every Init answers 0xBAD00001 — this is the most likely cause here{}.              Settings ▸ System ▸ Display ▸ Graphics ▸ Add a desktop app ▸ pick the game's exe              (and, for a 32-bit game, host64\\dlss5-feed-host64.exe) ▸ Options ▸ High performance.              Install sets that for you from this version on.", "此机器上有多于一个 GPU 厂商（{}），由 Windows 决定进程在哪个 GPU 上启动。若在内置 GPU 上启动，NGX 不存在，每次 Init 都会返回 0xBAD00001 —— 这是此处最可能的原因{}。设置 ▸ 系统 ▸ 显示 ▸ 图形 ▸ 添加桌面应用 ▸ 选择游戏 exe（32 位游戏还需 host64\\dlss5-feed-host64.exe）▸ 选项 ▸ 高性能。从本版本起安装会自动为你设置。",
            names.join(", "),
            if set {
                ", though the preference is already set to high performance for that exe"
            } else {
                ""
            }
        )));
    }
    let system = crate::ngx::describe();
    // Reported on three machines (RTX 4070, 5080, 5090) with NGX Core present and
    // driver 616.56, always on the Feeder's own in-process D3D12 device. The same
    // chain initialises NGX fine in the 32-bit host64 helper (a separate process)
    // and on the native path where the game owns the device, so the installed
    // files are not what decides it.
    let advice = if crate::ngx::healthy() {
        lang::tr("Your NGX runtime and driver are fine, so this is NGX refusing the Feeder's private          D3D12 device inside the game process, which has been reported on several machines.          Worth doing: install into a game that ships its own DLSS (that path opens no private          device) to confirm NGX works for you, then report this log at          github.com/jlrouzies-fr/DLSS5-Feeder, where that device is created.", "你的 NGX 运行时和驱动都没问题，因此这是 NGX 拒绝了 Feeder 在游戏进程内创建的私有 D3D12 设备，多台机器上都有报告。值得一试：装进一个自带 DLSS 的游戏（该路径不创建私有设备）以确认 NGX 对你有用，然后在 github.com/jlrouzies-fr/DLSS5-Feeder 报告这份日志（该设备就是在这里创建的）。")
    } else {
        lang::tr("Fix that first, then run Install again: reinstall the NVIDIA driver with a Custom          install that keeps every component (616.56 or newer).", "先修好它，再重新安装：用自定义安装重装 NVIDIA 驱动并保留所有组件（616.56 或更新）。")
    };
    out.push(bad(crate::trfmt!("NGX refused to initialise: {}. 0xBAD00001 is FeatureNotSupported, which NGX also          answers when its runtime is not on the system — not a ReShade, shader or add-on          problem. {system}. {advice}", "NGX 拒绝初始化：{}。0xBAD00001 是 FeatureNotSupported，当系统没有 NGX 运行时时 NGX 也会这样返回 —— 这不是 ReShade、着色器或附加组件的问题。{system}。{advice}",
        line.trim()
    )));
}

/// Newest Feeder known at build time; only used to nudge users off stale copies.
const CURRENT_FEEDER: &str = "0.12.0";

fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-'])
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

fn ok(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Ok,
        text: t.into(),
    }
}
fn warn(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Warn,
        text: t.into(),
    }
}
fn bad(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Bad,
        text: t.into(),
    }
}

fn read(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name)).ok()
}

/// The exe ReShade actually loaded into, from its first line:
/// `... loaded from '...dxgi.dll' into 'C:\\...bg3_dx11.exe' (0x...)`.
fn reshade_host_exe(log: &str) -> Option<String> {
    let line = log
        .lines()
        .find(|l| l.contains("loaded from") && l.contains(" into "))?;
    let path = line.split(" into ").nth(1)?.split('\'').nth(1)?;
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Findings for a game folder, in reading order.
pub fn diagnose(st: &GameStatus) -> Vec<Finding> {
    let d = st.game_dir();
    let mut out = Vec::new();
    let consumer = st.consumer_dir();
    let rs_log = read(&consumer, "ReShade.log").or_else(|| read(&consumer, "ReShade2.log"));
    // Wine and Proton substitute their own d3dcompiler_47.dll, whose HLSL
    // compiler is vkd3d-shader. It does not implement every attribute ReShade
    // emits, and says so in its own words (#70).
    let wine_hlsl = rs_log
        .as_deref()
        .is_some_and(|l| l.contains("not yet implemented feature"));

    // ── a game-shipped HLSL compiler shadowing the system one ──────
    // The add-on compiles its NR pass at cs_5_1. A d3dcompiler_47.dll that
    // ships with the game is loaded in preference to System32's, and an old
    // one does not know that target: "error X3506: unrecognized compiler
    // target" and no neural rendering, with everything else looking correct.
    let compiler = d.join("d3dcompiler_47.dll");
    if compiler.is_file() && !wine_hlsl {
        let ver = crate::ngx::file_version(&compiler).unwrap_or_else(|| "unknown".into());
        out.push(warn(crate::trfmt!("The game ships its own d3dcompiler_47.dll ({ver}), which Windows loads instead of \
             System32's. If it predates shader model 5.1 the DLSS 5 pass cannot compile \
             (error X3506). Rename it to d3dcompiler_47.dll.bak and start the game again; \
             almost every game runs fine on the system copy.", "游戏自带 d3dcompiler_47.dll（{ver}），Windows 会优先加载它而不是 System32 的版本。如果它早于着色器模型 5.1，DLSS 5 通道就无法编译（错误 X3506）。请将其重命名为 d3dcompiler_47.dll.bak 再启动游戏；几乎所有游戏都能用系统版本正常运行。"
        )));
    }

    // ── which neural model is installed ────────────────────
    // Two builds of nvngx_dlssnr.dll are in circulation and only the version
    // resource separates them; every failing RTX 50 report so far carries the
    // .SF one, so the log has to name it.
    for p in [d.join(game::DLSSNR_DLL), consumer.join(game::DLSSNR_DLL)] {
        if !p.is_file() {
            continue;
        }
        if let Some(v) = crate::ngx::file_version(&p) {
            out.push(ok(crate::trfmt!("DLSS 5 model {}: {v} — {}", "DLSS 5 模型 {}：{v} —— {}",
                if p.parent() == Some(d) {
                    lang::tr("beside the exe", "在 exe 旁")
                } else {
                    lang::tr("in host64", "在 host64 中")
                },
                crate::ngx::model_build(&v)
            )));
        }
        break;
    }

    // ── ReShade side ────────────────────────────────────────────────
    // The DLSS 5 add-on runs under the ReShade in `consumer_dir()`: beside the
    // exe for a 64-bit game, in host64\ for a 32-bit one. Reading the game
    // folder's log for a 32-bit game reads the *feeder's* 32-bit ReShade, which
    // never loads the add-on, so every 32-bit report came back "the add-on
    // never registered" no matter how healthy the install was (#69).
    let Some(rs) = rs_log else {
        out.push(bad(if st.is32() {
            "No host64\\ReShade.log: the 64-bit helper's ReShade never loaded, which is what \
             \"host lost: pipe never appeared\" in dlss5-feed.log means. Look in \
             host64\\dlss5-feed-host.log for the reason, and check antivirus did not remove \
             anything from host64\\."
        } else {
            lang::tr("No ReShade.log next to the game exe: ReShade never loaded. Either the game was not \
             started since the install, or it does not load dxgi.dll (wrong exe picked, or a \
             launcher starts a different one). Check the exe with --check.", "游戏 exe 旁没有 ReShade.log：ReShade 从未加载。要么安装后游戏没有启动过，要么它不加载 dxgi.dll（选错了 exe，或启动器启动的是另一个）。请用 --check 检查该 exe。")
        }));
        return out;
    };
    if rs.contains("Initializing crosire's ReShade") {
        out.push(ok(lang::tr("ReShade loaded into the game.", "ReShade 已加载到游戏中。")));
    }
    let failed_line = rs
        .lines()
        .find(|l| l.contains("Failed to load add-on") && l.contains("renodx-dlss5"));
    if let Some(l) = failed_line {
        let code = l
            .rsplit("error code ")
            .next()
            .unwrap_or("")
            .trim_end_matches('!');
        let extra = match code.trim() {
            "2148073478" => lang::tr(" (0x80090006 = the process refuses unsigned DLLs; nothing can be done)", "（0x80090006 = 进程拒绝未签名的 DLL；无能为力）"),
            "1114" => lang::tr(" (the add-on's DLL entry point failed; usually a CPU without AVX2 or a mismatched ReShade version)", "（附加组件的 DLL 入口点失败；通常是 CPU 缺少 AVX2 或 ReShade 版本不匹配）"),
            _ => "",
        };
        out.push(bad(crate::trfmt!("ReShade refused to load renodx-dlss5.addon64, error code {code}{extra}.", "ReShade 拒绝加载 renodx-dlss5.addon64，错误码 {code}{extra}。"
        )));
    } else if rs.contains("DLSS 5 Neural Rendering") {
        out.push(ok(lang::tr("The DLSS 5 Neural Rendering add-on registered.", "DLSS 5 神经渲染附加组件已注册。")));
    } else {
        out.push(bad(crate::trfmt!(
                "The DLSS 5 add-on never registered. renodx-dlss5.addon64 is missing from {}, disabled in ReShade's Add-ons tab, or quarantined by antivirus.",
                "DLSS 5 附加组件从未注册。renodx-dlss5.addon64 不在 {} 中、在 ReShade 的 Add-ons 标签页中被禁用，或被杀毒软件隔离。",
                if st.is32() {
                    lang::tr("host64\\ (where a 32-bit game's add-on lives)", "host64\\（32 位游戏的附加组件所在处）")
                } else {
                    lang::tr("the game folder", "游戏文件夹")
                }
            )));
    }
    if rs.contains("NR toggled ON") && !rs.contains("NR toggled OFF") {
        out.push(ok(lang::tr("Neural rendering was toggled ON (F6).", "神经渲染已开启（F6）。")));
    } else if rs.contains("NR toggled OFF") {
        out.push(warn(
            lang::tr("The log's last F6 state may be OFF — press F6 in game and watch the add-on's panel.", "日志中最后一次 F6 状态可能是关闭 —— 请在游戏中按 F6 并观察附加组件面板。"),
        ));
    }
    if rs.contains("inline feature 18 evaluation succeeded") {
        out.push(ok(
            lang::tr("Neural rendering ran: the add-on evaluated the DLSS 5 model on real frames. If the \
             picture still looks unchanged, raise NR Intensity / Local Structure in its panel — \
             the default is subtle.", "神经渲染已运行：附加组件在真实帧上评估了 DLSS 5 模型。如果画面仍无变化，请在其面板中调高 NR Intensity / Local Structure —— 默认效果很轻微。"),
        ));
    } else if rs.contains("feature=1 (DLSS/DLAA)") {
        out.push(warn(
            lang::tr("The add-on saw the game's DLSS but has not evaluated the model yet (feature 18 never \
             created). Enable DLSS in the game's own graphics settings and enable neural rendering \
             in the add-on panel.", "附加组件看到了游戏的 DLSS，但尚未评估模型（feature 18 从未创建）。请在游戏自身的图形设置中开启 DLSS，并在附加组件面板中开启神经渲染。"),
        ));
    } else if st.mode == game::Mode::Native {
        if st.feeder {
            out.push(warn(
                "Mode is Native DLSS (game ships its own DLSS), but dlss5-feed.addon64 is still \
                 present. Feeder Optimize does not apply here — NR is game/renodx. Run Remove \
                 (incl. Feeder leftovers) or Install again so Native cleanup drops the Feeder.",
            ));
        }
        // The add-on hooks NVSDK_NGX_D3D12_*. A game whose DLSS runs on D3D11
        // calls the D3D11 entry points, which it never sees, so "no create"
        // is expected until the bridge is installed (#33, BG3 DX11).
        if st.api == game::Api::Dx11 && !st.bridge {
            out.push(bad(
                lang::tr("No NGX call was intercepted, and this is a Direct3D 11 game with its own \
                 DLSS: the add-on hooks the D3D12 NGX entry points, but the game calls the \
                 D3D11 ones, so it can never see them. The DX11 bridge covers exactly this \
                 and is not installed here — run Install on this exe.", "没有拦截到 NGX 调用，而且这是一个自带 DLSS 的 Direct3D 11 游戏：附加组件挂钩的是 D3D12 NGX 入口点，但游戏调用的是 D3D11 入口点，因此它永远看不到。DX11 桥接正是为此而生，而这里没有安装 —— 请对此 exe 运行「安装」。"),
            ));
        } else {
            out.push(bad(
                lang::tr("No NGX call was intercepted: this game's own DLSS never ran. Turn DLSS on \
                 in the game's graphics settings (the add-on hooks the game's DLSS calls; \
                 without them it has nothing to work with).", "没有拦截到 NGX 调用：此游戏自身的 DLSS 从未运行。请在游戏的图形设置中开启 DLSS（附加组件挂钩游戏的 DLSS 调用；没有它们就没有可用的东西）。"),
            ));
        }
    }

    // Linux/Proton: ReShade generates HLSL with [fastopt] for shader model 4
    // and up, and Wine's d3dcompiler_47 (vkd3d-shader) has not implemented it,
    // so DLSS5_Feed.fx and the Lumenite shaders never build — with the feed
    // add-on then reporting its technique missing, which reads like our bug
    // rather than a missing compiler (#70).
    if wine_hlsl {
        let line = rs
            .lines()
            .find(|l| l.contains("not yet implemented feature"))
            .unwrap_or("")
            .trim();
        out.push(bad(format!(
            "The effects failed to compile in Wine/Proton's own HLSL compiler: {line} \
             That message comes from vkd3d-shader, which Wine's d3dcompiler_47.dll uses; \
             ReShade emits attributes it has not implemented. Install Microsoft's real \
             d3dcompiler_47 into the prefix — protontricks <appid> d3dcompiler_47, or \
             winetricks d3dcompiler_47 — and start the game again. If the game shipped its \
             own d3dcompiler_47.dll, leave it in place: under Proton it may be the only \
             working compiler there is."
        )));
    }

    // The compile failure itself, which is unambiguous when it appears.
    if let Some(line) = rs
        .lines()
        .find(|l| l.contains("X3506") || l.contains("unrecognized compiler target"))
    {
        out.push(bad(crate::trfmt!("{} — the HLSL compiler in this process is too old for the DLSS 5 pass. That is \
             a d3dcompiler_47.dll shipped with the game, loaded in preference to System32's. \
             Rename it (d3dcompiler_47.dll.bak) and start the game again.", "{} —— 此进程中的 HLSL 编译器对 DLSS 5 通道来说太旧。这是游戏自带的 d3dcompiler_47.dll，被优先于 System32 的版本加载。请重命名（d3dcompiler_47.dll.bak）再启动游戏。",
            line.trim()
        )));
    }

    // A game with more than one executable (a Vulkan build and a DX11 build,
    // a launcher and the game) can be installed for one and played through
    // another: ReShade loads, everything looks right, nothing is hooked (#33).
    if let Some(loaded) = reshade_host_exe(&rs).filter(|_| !st.is32()) {
        let ours = st
            .exe
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase());
        if ours.is_some_and(|o| o != loaded.to_ascii_lowercase()) {
            out.push(warn(crate::trfmt!("ReShade loaded into {loaded}, but this install was set up for {}. Those are \
                 different executables, and the install is tuned to the one you picked (the \
                 DX11 bridge in particular). Point the tool at {loaded} and run Install again.", "ReShade 加载进了 {loaded}，但本次安装是为 {} 配置的。这是两个不同的可执行文件，安装是针对你选择的那个调校的（尤其是 DX11 桥接）。请把工具指向 {loaded} 再重新安装。",
                st.exe.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
    }

    // ── DX11 bridge (native DLSS on D3D11) ──────────────────────────
    if let Some(bl) = read(d, "dlss5-bridge.log") {
        if let Some(line) = bl.lines().rev().find(|l| l.contains("stopped:")) {
            out.push(bad(crate::trfmt!("The DX11 bridge stopped: {}", "DX11 桥接已停止：{}", line.trim())));
        }
        if let Some(line) = bl
            .lines()
            .find(|l| l.contains("D3D12CreateDevice failed 0x887E0003"))
        {
            // Where the redist actually is decides what the user can do. Unreal
            // puts it in a D3D12 subfolder; a Unity player declares the exe's own
            // folder, so renaming a "D3D12 folder" that was never there changes
            // nothing and reads as a dead end (dlss5-bridge#24).
            let where_ = match game::has_agility_redist(d) {
                Some(p) => {
                    let ver = crate::ngx::file_version(&p).unwrap_or_else(|| "unknown".into());
                    crate::trfmt!("The copy in force here is {} ({ver}). Rename it and start the game \
                         again: it falls back to the Windows runtime, which every device in \
                         the process can match. If the game will not start without it, verify \
                         the game files instead -- a D3D12Core.dll replaced or truncated by \
                         another tool gives exactly this error.", "此处生效的副本是 {}（{ver}）。请重命名它再启动游戏：它会回退到 Windows 运行时，进程中的每个设备都能匹配。如果没了它游戏无法启动，请改为校验游戏文件 —— 被其它工具替换或截断的 D3D12Core.dll 恰好会导致这个错误。",
                        p.display()
                    )
                }
                None => lang::tr("No D3D12Core.dll is next to the exe or in a D3D12 folder here, so the \
                         declaration points somewhere else or the file is missing outright. \
                         Verify the game files.", "exe 旁或这里的 D3D12 文件夹中没有 D3D12Core.dll，因此该声明指向别处或文件完全缺失。请校验游戏文件。")
                    .into(),
            };
            out.push(bad(crate::trfmt!("{} — 0x887E0003 is D3D12_ERROR_INVALID_REDIST: the executable declares its own \
                 DirectX 12 Agility SDK (D3D12SDKVersion/D3D12SDKPath exports), and until that \
                 declaration is satisfied no D3D12 device can be created in this process at \
                 all -- not the bridge's, not the game's. Not something this tool sets. {}", "{} —— 0x887E0003 是 D3D12_ERROR_INVALID_REDIST：可执行文件声明了自己的 DirectX 12 Agility SDK（D3D12SDKVersion/D3D12SDKPath 导出），在满足该声明之前，此进程中根本无法创建任何 D3D12 设备 —— 桥接的也不行，游戏的也不行。这不是本工具设置的。{}",
                line.trim(),
                where_
            )));
        } else if bl.contains("frames:") && !bl.contains("session failed") {
            out.push(ok(
                lang::tr("The DX11 bridge opened its D3D12 session and is delivering frames.", "DX11 桥接已打开其 D3D12 会话并正在输出帧。"),
            ));
        }
    }

    // ── Feeder side (games without DLSS) ────────────────────────────
    if st.mode == game::Mode::Feeder {
        let Some(fd) = read(d, "dlss5-feed.log") else {
            out.push(bad(
                lang::tr("No dlss5-feed.log: DLSS5-Feeder never started. Its add-on is missing or disabled \
                 in ReShade's Add-ons tab.", "没有 dlss5-feed.log：DLSS5-Feeder 从未启动。其附加组件缺失，或在 ReShade 的 Add-ons 标签页中被禁用。"),
            ));
            return out;
        };
        if fd.contains("feature ready") {
            out.push(ok(
                lang::tr("DLSS5-Feeder built its DLSS feature (feature ready … DLAA).", "DLSS5-Feeder 已构建其 DLSS 特性（feature ready … DLAA）。"),
            ));
        }
        if fd.contains("frame") && fd.contains("delivered") {
            out.push(ok(lang::tr("Frames were delivered to the model.", "帧已交付给模型。")));
        }
        if fd.contains("technique MISSING") && !fd.contains("technique found") {
            out.push(bad(
                lang::tr("DLSS5_Feed.fx is not compiling. Its shader files are missing from \
                 reshade-shaders\\Shaders — re-run Install.", "DLSS5_Feed.fx 未能编译。其着色器文件从 reshade-shaders\\Shaders 中缺失 —— 请重新安装。"),
            ));
        }
        // The first effects line of a session always says "none": effects are
        // not compiled yet. Only the last one describes the running state (#6).
        let last_effects = fd
            .lines()
            .rev()
            .find(|l| l.contains("[feed] effects:"))
            .unwrap_or("");
        if last_effects.contains("-> none (not installed)") {
            out.push(bad(
                lang::tr("The motion-vector provider is not enabled. In ReShade's Home tab enable \
                 \"LUMENITE: Kernel 2.0\" ABOVE \"DLSS5_Feed\", then reload effects.", "运动矢量提供器未启用。请在 ReShade 的 Home 标签页中，把 \"LUMENITE: Kernel 2.0\" 启用在 \"DLSS5_Feed\" 上方，然后重新加载效果。"),
            ));
        }
        ngx_init_failure_for(&fd, Some(&st.exe), &mut out);
        // 32-bit games: the work happens in host64\, and its own log names the reason.
        if let Some(hl) = read(&d.join(game::HOST_DIR), "dlss5-feed-host.log") {
            if hl.contains("feature ready") {
                out.push(ok(
                    lang::tr("The host64 helper built its DLSS feature (feature ready … DLAA).", "host64 助手已构建其 DLSS 特性（feature ready … DLAA）。"),
                ));
            }
            ngx_init_failure_for(&hl, Some(&st.consumer_dir().join(game::HOST_EXE)), &mut out);
        } else if st.is32() {
            out.push(warn(
                lang::tr("No host64\\dlss5-feed-host.log yet: the 64-bit helper has not started. It is \
                 spawned by the first fed frame, so enable Lumenite_Kernel + DLSS5_Feed in \
                 ReShade's Home tab and play a moment first.", "还没有 host64\\dlss5-feed-host.log：64 位助手尚未启动。它由第一帧喂入时生成，因此请先在 ReShade 的 Home 标签页启用 Lumenite_Kernel + DLSS5_Feed 并玩一会儿。"),
            ));
        }
        if let Some(ver) = fd
            .lines()
            .next()
            .and_then(|l| {
                // "HH:MM:SS.mmm  dlss5-feed 0.12.0 (built ...) attached."
                let mut it = l.split_whitespace();
                it.find(|t| t.starts_with("dlss5-feed"))?;
                it.next()
            })
            .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        {
            if version_key(ver) < version_key(CURRENT_FEEDER) {
                out.push(warn(crate::trfmt!("DLSS5-Feeder {ver} in the log is older than {CURRENT_FEEDER}; re-run Install \
                     to refresh it (since 0.9.1 an existing Feeder is updated).", "日志中的 DLSS5-Feeder {ver} 比 {CURRENT_FEEDER} 旧；请重新安装以刷新（自 0.9.1 起会更新已有的 Feeder）。"
                )));
            }
        }
        if fd.contains("MV probe") && fd.contains("0% non-zero") {
            out.push(bad(
                lang::tr("Motion vectors are all zero: the provider is enabled but writes nothing. Check \
                 that Lumenite_Kernel sits above DLSS5_Feed in the technique list.", "运动矢量全为零：提供器已启用但什么都没写。请检查 Lumenite_Kernel 是否位于技术列表中的 DLSS5_Feed 上方。"),
            ));
        }
        if fd.contains("DLSS super sampling is not available") {
            out.push(bad(
                lang::tr("NGX reported DLSS unavailable. nvngx_dlss.dll must sit next to the game exe — \
                 re-run Install, and make sure antivirus did not remove it.", "NGX 报告 DLSS 不可用。nvngx_dlss.dll 必须放在游戏 exe 旁 —— 请重新安装，并确认杀毒软件没有删掉它。"),
            ));
        }
        // Some games refuse the reduced work-resolution path: the feed builds
        // its shared textures, the staging SRV for the smaller image fails, and
        // three failed builds stop the feed. Nothing downstream then happens —
        // the add-on's overlay says "HOOKS ARMED - NO DLSS CREATE SEEN" and
        // toggling neural rendering in game does nothing, which reads like the
        // add-on is broken rather than one setting being wrong (#74).
        if fd.contains("work-resolution staging SRV failed") {
            let pct = fd
                .lines()
                .find(|l| l.contains("work resolution ("))
                .and_then(|l| l.split("work resolution (").nth(1))
                .and_then(|r| r.split(')').next())
                .unwrap_or("below 100%")
                .to_owned();
            out.push(bad(format!(
                "The feed could not build its textures at {pct} of the frame: \
                 \"work-resolution staging SRV failed\", three times, and then it stopped. This \
                 game does not accept the reduced work-resolution path. Set it back to full size \
                 — Settings ▸ Feeder knobs ▸ work_resolution = 100 and work_upscale = 0, or pick \
                 the High quality preset — and start the game again. Everything downstream of this \
                 (no DLSS create, the in-game toggle doing nothing) follows from it."
            )));
        }
        if fd.contains("stopped:") {
            let line = fd
                .lines()
                .rev()
                .find(|l| l.contains("stopped:"))
                .unwrap_or("")
                .trim()
                .to_string();
            out.push(bad(crate::trfmt!("The feed stopped itself: {line}", "feed 自行停止：{line}")));
        }
        if fd.contains("CRASH RECORDED") {
            out.push(warn(
                lang::tr("The feed recorded a crash inside the DLSS 5 add-on (upstream issue #16). Play in \
                 borderless/windowed rather than exclusive fullscreen, and raise create_delay in \
                 dlss5-feed.cfg.", "feed 记录到 DLSS 5 附加组件内部崩溃（上游 issue #16）。请用无边框/窗口模式而非独占全屏游玩，并在 dlss5-feed.cfg 中调高 create_delay。"),
            ));
        }
    }
    out
}

/// Read the game folder and produce findings, or a single fatal one.
pub fn run(exe: &Path) -> Result<Vec<Finding>> {
    let st = game::inspect(exe)?;
    Ok(diagnose(&st))
}

#[cfg(test)]
mod tests {
    /// Baldur's Gate 3 ships bg3.exe (Vulkan) and bg3_dx11.exe; installing for
    /// one and playing the other leaves everything looking right and nothing
    /// hooked (#33). The exe name is in ReShade's first line.
    #[test]
    fn reshade_host_exe_reads_the_first_line() {
        let log = "01:30:11:810 [17792] | INFO  | Initializing crosire's ReShade version '6.8.0.2155' (64-bit) loaded from 'C:\\\\Program Files (x86)\\\\Steam\\\\steamapps\\\\common\\\\Baldurs Gate 3\\\\bin\\\\dxgi.dll' into 'C:\\\\Program Files (x86)\\\\Steam\\\\steamapps\\\\common\\\\Baldurs Gate 3\\\\bin\\\\bg3_dx11.exe' (0x64317982) ...";
        assert_eq!(
            super::reshade_host_exe(log).as_deref(),
            Some("bg3_dx11.exe")
        );
        assert!(super::reshade_host_exe("nothing useful here").is_none());
    }

    use super::*;
    use crate::game::testutil::*;

    fn setup(feeder: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X64);
        if !feeder {
            fs::write(t.path().join(game::DLSS_DLL), b"x").unwrap();
        }
        (t, exe)
    }

    /// A 32-bit game runs the add-on under the 64-bit ReShade in host64\, so
    /// that is the log to read. Reading the game folder's log — the feeder's
    /// own 32-bit ReShade, which never loads the add-on — reported "the add-on
    /// never registered" on installs that were fine (#69).
    #[test]
    fn thirty_two_bit_reads_the_host64_reshade_log() {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X86);
        let host = t.path().join(game::HOST_DIR);
        fs::create_dir_all(&host).unwrap();
        // The 32-bit ReShade beside the exe: no add-on, and never will have one.
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n",
        )
        .unwrap();
        // The one that matters, in host64\.
        fs::write(
            host.join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Ok && x.text.contains("add-on registered")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("never registered")),
            "{f:?}"
        );
    }

    /// And when host64\ has no ReShade log at all, say so in host64 terms
    /// rather than claiming ReShade never loaded beside the exe.
    #[test]
    fn thirty_two_bit_missing_host64_log_names_host64() {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X86);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f.iter().any(|x| x.text.contains("host64")), "{f:?}");
    }

    /// Under Proton the effects are compiled by Wine's d3dcompiler_47
    /// (vkd3d-shader), which has not implemented [fastopt]. Saying "the add-on
    /// never registered" or "rename your d3dcompiler" sends the user in the
    /// wrong direction — the second one actively removes the working compiler (#70).
    #[test]
    fn wine_hlsl_compiler_is_named_and_rename_advice_suppressed() {
        let (t, exe) = setup(true);
        fs::write(t.path().join("d3dcompiler_47.dll"), b"MZ").unwrap();
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n\
             ERROR | Failed to compile 'DLSS5_Feed.fx':\n\
             <anonymous>:118:13: E5017: Aborting due to not yet implemented feature: Unhandled attribute 'fastopt'.\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Bad && x.text.contains("vkd3d-shader")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("d3dcompiler_47.dll.bak")),
            "{f:?}"
        );
    }

    /// Dying Light refuses the reduced work-resolution path. The user sees a
    /// stopped feed and an add-on saying it never saw a DLSS create, with
    /// nothing naming the one setting responsible (#74).
    #[test]
    fn work_resolution_build_failure_names_the_setting() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] building: 2176x1224 work resolution (85%) -> 2560x1440 backbuffer\n\
             [feed] work-resolution staging SRV failed\n\
             [feed] failure: resource build\n\
             stopped: repeated failures. The game renders normally.\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        let hit = f
            .iter()
            .find(|x| x.text.contains("work-resolution staging SRV failed"))
            .unwrap_or_else(|| panic!("{f:?}"));
        assert_eq!(hit.level, Level::Bad);
        assert!(hit.text.contains("85%"), "{}", hit.text);
        assert!(hit.text.contains("work_resolution = 100"), "{}", hit.text);
    }

    #[test]
    fn no_reshade_log_is_fatal() {
        let (_t, exe) = setup(true);
        let f = run(&exe).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].level, Level::Bad);
        assert!(f[0].text.contains("never loaded"));
    }

    #[test]
    fn native_without_game_dlss_call() {
        let (t, exe) = setup(false);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f
            .iter()
            .any(|x| x.text.contains("game's own DLSS never ran")));
        assert!(f
            .iter()
            .any(|x| x.level == Level::Ok && x.text.contains("add-on registered")));
    }

    #[test]
    fn addon_load_failure_is_explained() {
        let (t, exe) = setup(false);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nFailed to load add-on from 'C:\\g\\renodx-dlss5.addon64' with error code 2148073478!\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f.iter().any(|x| x.text.contains("unsigned DLLs")));
    }

    #[test]
    fn feeder_provider_not_enabled() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] effects: DLSS5_Feed.fx technique found, DLSS5_MV_PROVIDER=3 (LumeniteFX Kernel) -> none (not installed)\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f
            .iter()
            .any(|x| x.text.contains("motion-vector provider is not enabled")));
    }

    #[test]
    fn feeder_last_effects_line_wins_and_ngx_init_failure_named() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "12:33:33.151  dlss5-feed 0.7.0 (built Aug 31 2026) attached.\n\
             [feed] effects: DLSS5_Feed.fx technique MISSING, DLSS5_MV_PROVIDER=3 (LumeniteFX Kernel) -> none (not installed)\n\
             [feed] effects: DLSS5_Feed.fx technique found, DLSS5_MV_PROVIDER=3 (LumeniteFX Kernel) -> Lumenite_Kernel (enabled)\n\
             [feed] NVSDK_NGX_D3D12_Init -> 0xBAD00001 (FeatureNotSupported)\n\
             stopped: the D3D12/NGX session failed to start.\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(!f
            .iter()
            .any(|x| x.text.contains("motion-vector provider is not enabled")));
        assert!(f
            .iter()
            .any(|x| x.text.contains("NGX refused to initialise")));
        assert!(f
            .iter()
            .any(|x| x.text.contains("0.7.0 in the log is older")));
    }

    #[test]
    fn healthy_session_says_so() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\ninline feature 18 evaluation succeeded (count=60)\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] feature ready: 3840x2160 DLAA\n[feed] frame 1 delivered\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f.iter().all(|x| x.level == Level::Ok), "{f:?}");
        assert!(f.iter().any(|x| x.text.contains("raise NR Intensity")));
    }
}
