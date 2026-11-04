fn main() {
    for url in std::env::args().skip(1) {
        match launcher_core::download::fetch_file_bytes(&url, &Default::default()) {
            Ok(bytes) => match launcher_core::icons::inspect(&bytes) {
                Some((mime, blank)) => println!(
                    "{} -> {} bytes, mime={mime}, blank={blank}",
                    &url[url.len().saturating_sub(28)..],
                    bytes.len()
                ),
                None => println!("{} -> 无法解码", &url[url.len().saturating_sub(28)..]),
            },
            Err(error) => println!("{} -> error: {error}", &url[url.len().saturating_sub(28)..]),
        }
    }
}
