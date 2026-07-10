pub fn normalize_ioc(ioc_type: &str, value: &str) -> String {
    match ioc_type {
        "domain" | "url" | "email-addr" => value.trim().to_lowercase(),
        "ipv4" | "ipv6" => value.trim().to_string(),
        _ => value.trim().to_string(),
    }
}
