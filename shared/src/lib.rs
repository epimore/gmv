#![allow(warnings)]
pub mod info;
pub mod io;

pub use paste;

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }


    #[test]
    fn test_end_with() {
        let mut params = HashMap::new();
        params.insert("fileId".to_string(), "123".to_string());
        params.insert("user_f1ileId".to_string(), "123".to_string());
        params.insert("document_file1ID".to_string(), "456".to_string());

        let id = params
            .iter()
            .find(|(key, _)| key.to_lowercase().ends_with("fileid"))
            .map(|(_, value)| value);
        println!("{:?}", id);
    }
}
