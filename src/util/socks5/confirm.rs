pub struct Confirm {
    status: u8,
    port0: u8,
    port1: u8,
}
impl Confirm {
    pub fn new(error: bool, port: u16) -> Self {
        let port = port.to_be_bytes();
        if error {
            Self { status: 1, port0: port[0], port1: port[1] }
        } else {
            Self { status: 0, port0: port[0], port1: port[1] }
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        [0x05, self.status, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff].to_vec()
    }
}