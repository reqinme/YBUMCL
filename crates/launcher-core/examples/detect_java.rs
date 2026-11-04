use std::path::PathBuf;
fn main() {
    println!("JAVA_HOME={:?}", std::env::var_os("JAVA_HOME"));
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        cands.push(PathBuf::from(home).join("bin/java.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            cands.push(dir.join("java.exe"));
        }
    }
    let mut n = 0;
    for c in &cands {
        let is_file = c.is_file();
        let canon = std::fs::canonicalize(c).ok();
        if is_file || canon.is_some() {
            n += 1;
            println!("cand {:?} file={} canon={:?}", c, is_file, canon);
        }
    }
    println!("candidates present: {n}");
    let found = launcher_core::detect_java();
    println!("found {}", found.len());
    for j in &found {
        println!("  {:<6} {:<40} {}", j.major, j.version, j.path);
    }
}
