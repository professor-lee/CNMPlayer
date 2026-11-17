use super::*;

#[test]
fn test_cookie_to_json() {
    let cookie = "name=value; foo=bar; empty=";
    let map = cookie_to_json(cookie);
    assert_eq!(map.get("name").unwrap(), "value");
    assert_eq!(map.get("foo").unwrap(), "bar");
    assert_eq!(map.get("empty").unwrap(), "");
}

#[test]
fn test_cookie_to_json_empty() {
    let map = cookie_to_json("");
    assert!(map.is_empty());
}

#[test]
fn test_cookie_obj_to_string() {
    let mut map = HashMap::new();
    map.insert("name".to_string(), "value".to_string());
    let s = cookie_obj_to_string(&map);
    assert!(s.contains("name=value"));
}

#[test]
fn jar_replace_drops_set_cookie_attributes_from_legacy_strings() {
    let mut jar = CookieJar::default();
    jar.replace(
        "MUSIC_U=abc; Max-Age=1296000; Expires=Thu, 01 Dec 2026 00:00:00 GMT; Path=/; HTTPOnly; \
         __csrf=def; path=/",
    );
    assert_eq!(jar.header(), "MUSIC_U=abc; __csrf=def");
}

#[test]
fn jar_merges_set_cookie_and_honours_deletion() {
    let mut jar = CookieJar::default();
    jar.replace("MUSIC_U=abc; __csrf=old");
    jar.apply_set_cookie("__csrf=new; Max-Age=1296000; Path=/");
    jar.apply_set_cookie("NMTID=00X; Path=/");
    assert_eq!(jar.header(), "MUSIC_U=abc; __csrf=new; NMTID=00X");

    jar.apply_set_cookie("NMTID=; Max-Age=0; Path=/");
    jar.apply_set_cookie("MUSIC_U=; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/");
    assert_eq!(jar.header(), "__csrf=new");
}

#[test]
fn jar_replace_bumps_epoch_and_empty_string_clears() {
    let mut jar = CookieJar::default();
    let before = jar.epoch();
    jar.replace("MUSIC_U=abc");
    assert_ne!(jar.epoch(), before);
    jar.replace("");
    assert!(jar.is_empty());
}
