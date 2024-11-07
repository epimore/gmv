#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hyper::Uri;

    #[test]
    fn test_map() {
        let mut empty_map = HashMap::<String, u8>::new();
        let res = empty_map.get("a").or_else(||empty_map.get("b")).or_else(||empty_map.get("c"));
        assert_eq!(res, None);

        let mut a_map = HashMap::<String, u8>::new();
        a_map.insert("a".to_string(),1);
        let res = a_map.get("a").or_else(||a_map.get("b")).or_else(||a_map.get("c"));
        assert_eq!(res, Some(&1));

        let mut b_map = HashMap::<String, u8>::new();
        b_map.insert("b".to_string(),2);
        let res = b_map.get("a").or_else(||b_map.get("b")).or_else(||b_map.get("c"));
        assert_eq!(res, Some(&2));


        let mut c_map = HashMap::<String, u8>::new();
        c_map.insert("c".to_string(), 3);

        let res = c_map.get("a")
            .or_else(|| c_map.get("b"))
            .or_else(|| c_map.get("c"));

        assert_eq!(res, Some(&3));
    }

    #[test]
    fn test_url(){
        let uri: Uri = "http://example.org.cn/hello/world531537_1727574835779.flv?gmv_token=adaf1231241a&aaa=bbb".parse().unwrap();
        println!("{}", uri.path());
    }
}