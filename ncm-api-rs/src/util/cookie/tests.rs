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
