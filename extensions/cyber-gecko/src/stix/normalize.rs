use std::net::{Ipv4Addr, Ipv6Addr};

/// Normalizes an IOC value to a canonical form so the same indicator from
/// different feeds resolves to a single graph entity. Defanged notations
/// (`hxxp`, `[.]`, `(dot)`, `[at]`) are refanged first, then each IOC type is
/// canonicalized according to which of its parts are case-insensitive.
pub fn normalize_ioc(ioc_type: &str, value: &str) -> String {
    let v = refang(value);
    let v = v.trim();
    match ioc_type {
        "domain" => v.trim_end_matches('.').to_lowercase(),
        "ipv4" => v
            .parse::<Ipv4Addr>()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| v.to_lowercase()),
        "ipv6" => v
            .parse::<Ipv6Addr>()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| v.to_lowercase()),
        "url" => normalize_url(v),
        "email-addr" => normalize_email(v),
        "file-sha256" | "file-sha1" | "file-md5" => v.to_lowercase(),
        // Mutex names and registry keys are case-sensitive; keep them verbatim.
        _ => v.to_string(),
    }
}

/// Reverses the common defanging conventions CTI feeds apply to IOCs, so a
/// defanged value canonicalizes identically to its live form.
fn refang(value: &str) -> String {
    let mut s = value.to_string();
    for (from, to) in [
        ("[.]", "."),
        ("(.)", "."),
        ("{.}", "."),
        ("[dot]", "."),
        ("(dot)", "."),
        ("[:]", ":"),
        ("[at]", "@"),
        ("(at)", "@"),
        ("[@]", "@"),
        ("hxxps", "https"),
        ("hxxp", "http"),
        ("hXXps", "https"),
        ("hXXp", "http"),
    ] {
        s = s.replace(from, to);
    }
    s
}

/// Lowercases a URL's scheme and authority (case-insensitive per RFC 3986) while
/// preserving the path, query, and fragment, which are case-sensitive.
fn normalize_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, url),
    };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(host_end);
    let authority = authority.to_lowercase();
    match scheme {
        Some(s) => format!("{}://{authority}{tail}", s.to_lowercase()),
        None => format!("{authority}{tail}"),
    }
}

/// Lowercases an email's domain (case-insensitive) while preserving the
/// local-part, which is case-sensitive per RFC 5321.
fn normalize_email(email: &str) -> String {
    match email.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => email.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_is_lowercased_and_stripped_of_trailing_dot() {
        assert_eq!(normalize_ioc("domain", "Evil.COM."), "evil.com");
        assert_eq!(normalize_ioc("domain", "evil[.]com"), "evil.com");
    }

    #[test]
    fn ipv4_is_canonicalized() {
        assert_eq!(normalize_ioc("ipv4", " 1.1.1[.]1 "), "1.1.1.1");
    }

    #[test]
    fn ipv6_is_zero_compressed_and_lowercased() {
        assert_eq!(
            normalize_ioc("ipv6", "2001:0DB8:0000:0000:0000:0000:0000:0001"),
            "2001:db8::1"
        );
    }

    #[test]
    fn url_lowercases_scheme_and_host_but_preserves_path() {
        assert_eq!(
            normalize_ioc("url", "hXXps://Evil.COM/PaTh?Q=1"),
            "https://evil.com/PaTh?Q=1"
        );
    }

    #[test]
    fn email_lowercases_domain_only() {
        assert_eq!(
            normalize_ioc("email-addr", "Admin[at]Evil.COM"),
            "Admin@evil.com"
        );
    }

    #[test]
    fn hashes_are_lowercased() {
        assert_eq!(normalize_ioc("file-sha256", "ABCDEF01"), "abcdef01");
    }

    #[test]
    fn mutex_is_case_preserved() {
        assert_eq!(normalize_ioc("mutex", "Global\\MyMutex"), "Global\\MyMutex");
    }
}
