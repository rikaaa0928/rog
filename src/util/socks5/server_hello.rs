pub struct ServerHello {
    version: u8,
    method: u8,
}

impl ServerHello {
    pub fn new(version: u8, method: u8) -> Self {
        ServerHello {
            version,
            method,
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut res = Vec::new();
        res.push(self.version);
        res.push(self.method);
        res
    }
}