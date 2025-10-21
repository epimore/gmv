struct LocalMp4 {
    path: String,
    file_name: String,
    file_size: Option<u64>,
    timestamp: u32,
}
impl LocalMp4 {

    pub fn store(ssrc:u32){

    }
    fn new(path: String, file_name: String) -> Self {
        Self {
            path,
            file_name,
            file_size: None,
            timestamp: 0,
        }
    }

}
