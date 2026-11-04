use std::time::Duration;

fn main() {
    let url = std::env::args().nth(1).expect("url");
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .http1_only()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(120))
        .local_address(Some(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)))
        .build()
        .unwrap();
    match client.get(&url).send() {
        Ok(response) => println!(
            "ok {} bytes={:?}",
            response.status(),
            response.bytes().map(|b| b.len())
        ),
        Err(error) => println!("err {error:?}"),
    }
}
