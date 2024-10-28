pub struct Confirm {
    status: u8,
}
impl Confirm {
    pub fn new(error: bool) -> Self {
        if error {
            Self { status: 1 }
        } else {
            Self { status: 0 }
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        [0x05, self.status, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff].to_vec()
    }
}