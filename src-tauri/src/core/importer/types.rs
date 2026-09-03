struct ImportedSession {
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    group_path: Option<Vec<String>>,
    description: Option<String>,
}

#[derive(Debug)]
struct PreparedJsonConnection {
    name: String,
    config: ConnectionType,
    group_path: Option<Vec<String>>,
    description: Option<String>,
    sort_order: i32,
    icon: Option<String>,
    auth: Option<ConnectionAuth>,
}

#[derive(Debug)]
struct PreparedJsonImport {
    groups: Vec<Vec<String>>,
    passwords: Vec<config::SavedPassword>,
    ssh_keys: Vec<config::SshKey>,
    connections: Vec<PreparedJsonConnection>,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonImportFile {
    #[serde(default = "default_import_version")]
    version: u32,
    #[serde(default)]
    passwords: Vec<NyatermJsonPassword>,
    #[serde(default)]
    ssh_keys: Vec<NyatermJsonSshKey>,
    #[serde(default)]
    groups: Vec<NyatermJsonGroup>,
    #[serde(default)]
    sessions: Vec<NyatermJsonSession>,
}

fn default_import_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct NyatermJsonPassword {
    #[serde(rename = "ref")]
    ref_name: String,
    name: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonSshKey {
    #[serde(rename = "ref")]
    ref_name: String,
    name: String,
    private_key: String,
    #[serde(default)]
    certificate: Option<String>,
    #[serde(default)]
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonGroup {
    path: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonSshAuth {
    mode: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    password_ref: Option<String>,
    #[serde(default)]
    key_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NyatermJsonSession {
    Ssh {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        #[serde(default = "default_ssh_user")]
        username: String,
        #[serde(default)]
        auth: Option<NyatermJsonSshAuth>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        dynamic_tab_title: bool,
    },
    LocalTerminal {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        #[serde(default)]
        shell_path: String,
        #[serde(default)]
        shell_args: String,
        #[serde(default)]
        working_dir: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        dynamic_tab_title: bool,
    },
    Telnet {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        host: String,
        #[serde(default = "default_telnet_port")]
        port: u16,
        #[serde(default = "default_telnet_backspace_mode")]
        backspace_mode: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    Serial {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        port_name: String,
        #[serde(default = "default_serial_baud_rate")]
        baud_rate: u32,
        #[serde(default = "default_serial_data_bits")]
        data_bits: u8,
        #[serde(default = "default_serial_parity")]
        parity: String,
        #[serde(default = "default_serial_stop_bits")]
        stop_bits: String,
        #[serde(default = "default_serial_backspace_mode")]
        backspace_mode: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
}

fn default_ssh_port() -> u16 {
    22
}

fn default_ssh_user() -> String {
    "root".to_string()
}

fn default_telnet_port() -> u16 {
    23
}

fn default_telnet_backspace_mode() -> String {
    "del".to_string()
}

fn default_serial_baud_rate() -> u32 {
    115_200
}

fn default_serial_data_bits() -> u8 {
    8
}

fn default_serial_parity() -> String {
    "none".to_string()
}

fn default_serial_stop_bits() -> String {
    "1".to_string()
}

fn default_serial_backspace_mode() -> String {
    "ctrl_h".to_string()
}

