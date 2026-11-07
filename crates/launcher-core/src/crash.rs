//! 崩溃报告的诊断：从原文里认出几类高置信度的原因，给一句人话结论和几条建议。
//!
//! 只做「看一眼就能确定」的那几类（PCL 的 `ModCrash` 有 46 类，这里取其中误判率最低的
//! 一部分）：认不出来就返回空，界面上照常给出原文，不猜。

use serde::Serialize;

/// 一条诊断结论。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    /// 稳定的标识，界面/日志里用得到，也方便以后接规则开关。
    pub key: String,
    /// 一句话结论。
    pub title: String,
    /// 依据：命中的原文片段或解释。
    pub detail: String,
    /// 可执行的建议，按顺序给。
    pub advice: Vec<String>,
}

fn hit(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

/// 从报告原文里认出原因；同一类只出一次，最多三条。
pub fn diagnose(text: &str) -> Vec<Diagnosis> {
    let lower = text.to_ascii_lowercase();
    let mut found: Vec<Diagnosis> = Vec::new();
    let mut push = |item: Diagnosis| {
        if found.len() < 3 && !found.iter().any(|known| known.key == item.key) {
            found.push(item);
        }
    };

    // 内存：堆或元空间撑爆，是单人整合包最常见的一类。
    if hit(&lower, "java.lang.outofmemoryerror") || hit(&lower, "java heap space") {
        push(Diagnosis {
            key: "out-of-memory".into(),
            title: "游戏内存不够".into(),
            detail: "日志里有 OutOfMemoryError，给这个实例分的内存装不下整合包或光影。"
                .into(),
            advice: vec![
                "在实例设置里把内存调大一些，比如整合包给 4096–8192 MB".into(),
                "同时开着浏览器、录屏之类的程序时先关掉".into(),
                "内存加到很大还是崩，多半是某个模组在漏内存".into(),
            ],
        });
    }

    // 内存参数本身不合法：32 位 Java 或者数字写得离谱。
    if hit(&lower, "invalid maximum heap size")
        || hit(&lower, "could not reserve enough space for object heap")
    {
        push(Diagnosis {
            key: "heap-size".into(),
            title: "分配的内存大小游戏没法接受".into(),
            detail: "JVM 起不来，说明 -Xmx 的数值超过了这个 Java 能用的上限。".into(),
            advice: vec![
                "确认用的是 64 位 Java（32 位最多只能分到 1.5 GB 左右）".into(),
                "把实例内存改回 2048–4096 MB 再试".into(),
            ],
        });
    }

    // Java 版本太新：老 Forge 的类文件版本对不上。
    if hit(&lower, "unsupportedclassversionerror")
        || hit(&lower, "has been compiled by a more recent version of the java runtime")
    {
        push(Diagnosis {
            key: "java-too-old".into(),
            title: "Java 版本比游戏要求的低".into(),
            detail: "日志里有 UnsupportedClassVersionError：游戏或模组是用更新的 Java 编译的，现在这个 Java 读不了。".into(),
            advice: vec![
                "在实例设置里换一个更新的 Java：1.17 以后要 Java 17+，1.20.5 以后要 Java 21".into(),
                "换完再启动一次，不用重装实例".into(),
            ],
        });
    }

    // Java 版本太旧：Forge/NeoForge 自带的那句提示。
    if hit(&lower, "unsupported java, your java is")
        || hit(&lower, "java.lang.unsupportedoperationexception: java")
    {
        push(Diagnosis {
            key: "java-unsupported".into(),
            title: "这个 Java 版本加载器不支持".into(),
            detail: "加载器明确报了 Unsupported Java：它只认特定大版本的 Java。".into(),
            advice: vec![
                "老版本 Forge（1.12.2 那会儿）要用 Java 8".into(),
                "1.16.5–1.17 用 Java 8 或 11，1.18 以后用 17，新版本用 21".into(),
            ],
        });
    }

    // 显卡/驱动不支持 OpenGL 3.2，Minecraft 起不来窗口。
    if hit(&lower, "glfw error")
        || hit(&lower, "pixel format not accelerated")
        || hit(&lower, "the driver does not appear to support opengl")
        || hit(&lower, "failed to create window")
    {
        push(Diagnosis {
            key: "opengl".into(),
            title: "显卡驱动不支持游戏要的 OpenGL".into(),
            detail: "日志里有 GLFW/OpenGL 报错，窗口都没能建出来。".into(),
            advice: vec![
                "去显卡官网更新驱动（集显也要更新对应厂商的）".into(),
                "远程桌面、虚拟机里通常没有 OpenGL，换成在本机直接跑".into(),
                "装光影或 Shader 相关的模组时，先确认驱动支持 OpenGL 3.2 以上".into(),
            ],
        });
    }

    // Fabric：两个模组提供同一个 mod id。
    if hit(&lower, "duplicatemodsfoundexception") {
        push(Diagnosis {
            key: "duplicate-mod".into(),
            title: "同一个模组装了两份".into(),
            detail: "Fabric 报了 DuplicateModsFoundException：mods 目录里有同 id 的不同文件。".into(),
            advice: vec![
                "打开 mods 目录，把同名的旧版本删掉，只留一份".into(),
                "整合包自带的模组和自己加的有重复时，优先保留整合包里的那份".into(),
            ],
        });
    }

    // Fabric：缺前置。
    if hit(&lower, "missing or unsupported mandatory dependencies")
        || hit(&lower, "missingdependencyexception")
        || hit(&lower, "missing mods:")
        || hit(&lower, "mod resolution encountered an incompatible mod set")
    {
        push(Diagnosis {
            key: "missing-dependency".into(),
            title: "少了模组需要的前置".into(),
            detail: "加载器报了缺失的依赖，通常是 Fabric API 或者某个库模组没装。".into(),
            advice: vec![
                "按日志里列出的 mod id 去下载页搜同名模组补齐".into(),
                "装模组时留意它标注的依赖，通用前置（Fabric API、Architectury 等）要先装".into(),
            ],
        });
    }

    // 压缩包损坏：模组/整合包被解压过、下载中断过。
    if hit(&lower, "java.util.zip.zipexception")
        || hit(&lower, "invalid loc header")
        || hit(&lower, "invalid cen header")
        || hit(&lower, "zip end header not found")
        || hit(&lower, "not in gzip format")
    {
        push(Diagnosis {
            key: "broken-jar".into(),
            title: "有个文件损坏了".into(),
            detail: "日志里是 ZipException：某个 jar 下载不完整，或者被解压工具改过。".into(),
            advice: vec![
                "看报错路径，把那个模组/资源包重新下一次".into(),
                "模组不要先解压再放回去，直接放 .jar 文件".into(),
                "整合包安装失败过的话，重新装一遍整合包".into(),
            ],
        });
    }

    // 类找不到：常见于 mixin 或加载器与游戏版本对不上。
    if hit(&lower, "noclassdeffounderror")
        && (hit(&lower, "mixin") || hit(&lower, "net/minecraft") || hit(&lower, "net.minecraft"))
    {
        push(Diagnosis {
            key: "missing-class".into(),
            title: "有类加载不到，模组和加载器版本对不上".into(),
            detail: "日志里是 NoClassDefFoundError，且指向游戏或 mixin 的类。".into(),
            advice: vec![
                "确认这个模组支持的 Minecraft 版本和加载器版本与实例一致".into(),
                "加载器和模组一起升级；只升其中一个很容易出这种错".into(),
            ],
        });
    }

    // 方法/字段签名变了：模组按旧版游戏编译。
    if hit(&lower, "java.lang.nosuchmethoderror") || hit(&lower, "java.lang.nosuchfielderror") {
        push(Diagnosis {
            key: "incompatible-mod".into(),
            title: "模组和这个游戏版本不兼容".into(),
            detail: "日志里是 NoSuchMethodError / NoSuchFieldError：模组调用的游戏方法是别的版本才有的。"
                .into(),
            advice: vec![
                "去模组页面找对应游戏版本的构建，或者等它更新".into(),
                "成批出现时，多半是某个前置模组版本不对，先更新前置".into(),
            ],
        });
    }

    // 磁盘空间。
    if hit(&lower, "not enough space")
        || hit(&lower, "no space left on device")
        || hit(&lower, "there is not enough space on the disk")
    {
        push(Diagnosis {
            key: "no-space".into(),
            title: "磁盘空间不够".into(),
            detail: "写日志或解压时系统报了空间不足。".into(),
            advice: vec![
                "清一下磁盘，尤其是游戏目录所在的盘".into(),
                "工具页的「清理」能腾出缓存和日志占的地方".into(),
            ],
        });
    }

    // 权限/占用。
    if hit(&lower, "accessdeniedexception")
        || hit(&lower, "access is denied")
        || hit(&lower, "另一个程序正在使用此文件")
    {
        push(Diagnosis {
            key: "access-denied".into(),
            title: "文件被占用或者没有写权限".into(),
            detail: "日志里出现了拒绝访问。".into(),
            advice: vec![
                "先确认游戏没有在后台开着（任务管理器里看 java 进程）".into(),
                "把游戏目录从杀毒软件的实时扫描里排除".into(),
            ],
        });
    }

    // 网络：登录或资源下载失败。
    if hit(&lower, "unknownhostexception")
        || hit(&lower, "connection reset")
        || hit(&lower, "failed to login")
        || hit(&lower, "unable to access")
    {
        push(Diagnosis {
            key: "network".into(),
            title: "连不上服务器".into(),
            detail: "日志里有域名解析或连接被重置的报错，通常是启动时的登录/资源请求失败。".into(),
            advice: vec![
                "换个网络或者稍后再试，这类错误多半是暂时的".into(),
                "用了代理/加速器的话先关掉再启动".into(),
            ],
        });
    }

    // 最后兜底：crash report 自带的那行描述。
    // 游戏常常只写一句 `Description: Unexpected error`，那就从堆栈里挖线索。
    if let Some(reason) = crash_reason(text) {
        let vague = reason.trim().is_empty()
            || reason.to_lowercase().contains("unexpected")
            || reason.to_lowercase().contains("unknown");
        let mut advice = Vec::new();
        if let Some(cause) = caused_by(text) {
            advice.push(format!("Caused by: {cause}"));
        }
        let frames = suspect_frames(text);
        if !frames.is_empty() {
            advice.push(format!("可疑位置：{}", frames.join("、")));
        }
        if advice.is_empty() || !vague {
            advice.push("先翻原文里第一段堆栈，看是哪个模组抛的".into());
        }
        if vague {
            advice.push("不确定就先移除最近装的模组，再一个个加回来".into());
        }
        push(Diagnosis {
            key: "crash-report".into(),
            title: "崩溃报告里的原因".into(),
            detail: reason,
            advice,
        });
    }

    found
}

/// 第一条 `Caused by:`：比 `Description:` 具体得多。
fn caused_by(text: &str) -> Option<String> {
    for line in text.lines().take(400) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Caused by:") {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(rest.chars().take(160).collect());
            }
        }
    }
    None
}

/// 堆栈里不像原版/加载器的帧，去掉包名只留类，最多三个。
fn suspect_frames(text: &str) -> Vec<String> {
    const SKIP: [&str; 19] = [
        "java.",
        "javax.",
        "jdk.",
        "sun.",
        "com.sun.",
        "net.minecraft.",
        "com.mojang.",
        "cpw.mods.",
        "net.minecraftforge.",
        "org.spongepowered.",
        "org.lwjgl.",
        "it.unimi.",
        "com.google.",
        "org.apache.",
        "io.netty.",
        "kotlin.",
        "knot",
        "net.fabricmc.",
        "org.jetbrains.",
    ];
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().take(400) {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("at ") else {
            continue;
        };
        let class = rest.split('(').next().unwrap_or(rest).trim();
        let class = class.split(" / ").next().unwrap_or(class).trim();
        let Some(first) = class.split('.').next() else {
            continue;
        };
        if SKIP.iter().any(|prefix| class.starts_with(prefix)) {
            continue;
        }
        // 只认像包名的（至少两段），避免把 `at Native Method` 之类算进来。
        if !class.contains('.') || first.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            continue;
        }
        // 混淆名（`net.minecraft.class_761`）对人没用，跳过。
        if class.contains(".class_") {
            continue;
        }
        if out.iter().any(|seen| seen == class) {
            continue;
        }
        out.push(class.to_string());
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// 崩溃报告开头那几行：`Description: ...` 或 `The game crashed whilst ...`。
fn crash_reason(text: &str) -> Option<String> {
    for line in text.lines().take(80) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Description:") {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(rest.to_owned());
            }
        }
        if line.starts_with("The game crashed whilst") {
            return Some(line.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oom_is_recognised() {
        let text = "java.lang.OutOfMemoryError: Java heap space\n\tat net.minecraft.client.Main.main";
        let found = diagnose(text);
        assert_eq!(found[0].key, "out-of-memory");
        assert!(!found[0].advice.is_empty());
    }

    #[test]
    fn broken_jar_is_recognised() {
        let text = "Caused by: java.util.zip.ZipException: invalid LOC header (bad signature)";
        assert!(diagnose(text).iter().any(|item| item.key == "broken-jar"));
    }

    #[test]
    fn duplicate_mods_is_recognised() {
        let text = "net.fabricmc.loader.impl.FormattedException: DuplicateModsFoundException";
        assert!(diagnose(text).iter().any(|item| item.key == "duplicate-mod"));
    }

    #[test]
    fn vague_description_lists_suspect_frames() {
        let text = "---- Minecraft Crash Report ----

Description: Unexpected error

java.lang.RuntimeException: boom
	at com.simibubi.create.content.Create.onInitialize(Create.java:120)
	at net.minecraft.client.Minecraft.run(Minecraft.java:900)
";
        let found = diagnose(text);
        let item = found.iter().find(|d| d.key == "crash-report").expect("fallback");
        assert!(item.advice.iter().any(|line| line.contains("com.simibubi.create")));
        assert!(!item.advice.iter().any(|line| line.contains("net.minecraft.client")));
    }

    #[test]
    fn caused_by_is_surfaced() {
        let text = "Description: Unexpected error

java.lang.RuntimeException: wrap
	Caused by: java.lang.IllegalStateException: bad state
";
        let found = diagnose(text);
        let item = found.iter().find(|d| d.key == "crash-report").expect("fallback");
        assert!(item.advice.iter().any(|line| line.starts_with("Caused by:")));
    }

    #[test]
    fn crash_reason_falls_back_to_description() {
        let text = "---- Minecraft Crash Report ----\n// Ooh. Shiny.\n\nDescription: Rendering overlay\n\njava.lang.NullPointerException";
        let found = diagnose(text);
        assert_eq!(found[0].key, "crash-report");
        assert_eq!(found[0].detail, "Rendering overlay");
    }

    #[test]
    fn unknown_text_gives_nothing() {
        assert!(diagnose("一切正常，没有崩溃").is_empty());
    }

    #[test]
    fn at_most_three_and_unique() {
        let text = "java.lang.OutOfMemoryError\njava.lang.OutOfMemoryError: Java heap space\n\
                    DuplicateModsFoundException\njava.util.zip.ZipException\nNoClassDefFoundError mixin";
        let found = diagnose(text);
        assert!(found.len() <= 3);
        let mut keys: Vec<_> = found.iter().map(|item| item.key.clone()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), found.len());
    }
}
