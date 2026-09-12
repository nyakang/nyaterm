pub struct SerialConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
    pub name: String,
    pub backspace_mode: String,
    pub modem_upload_protocol: crate::config::SerialModemUploadProtocol,
    pub encoding: String,
}
