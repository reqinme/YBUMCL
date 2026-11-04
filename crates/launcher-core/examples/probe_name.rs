fn main() {
    for slug in ["sodium", "jei", "create", "iris"] {
        println!("{slug} => {:?}", launcher_core::names::chinese_name(slug));
    }
    let page = launcher_core::modrinth::search_page(
        "mods",
        "sodium",
        None,
        None,
        "relevance",
        None,
        None,
        5,
        0,
    );
    match page {
        Ok(page) => {
            for item in page.items.iter().take(3) {
                println!(
                    "{} | {:?} | {:?} | {} versions",
                    item.title,
                    item.chinese_title,
                    item.loaders,
                    item.game_versions.len()
                );
            }
        }
        Err(error) => println!("search failed: {error}"),
    }
}
