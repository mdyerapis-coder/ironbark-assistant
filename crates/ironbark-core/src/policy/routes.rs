use url::Url;

fn destination(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty() || url.password().is_some()
        || url.fragment().is_some() || url.host_str().is_none() {
        return None;
    }
    Some(url)
}

pub fn is_loopback_destination(value: &str) -> bool {
    let Some(url) = destination(value) else { return false; };
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        _ => false,
    }
}

pub fn destination_allowed(value: &str, allowed: &[String]) -> bool {
    let Some(requested) = destination(value) else { return false; };
    allowed.iter().any(|candidate| destination(candidate).is_some_and(|candidate| candidate == requested))
}
