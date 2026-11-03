/// 配置常量 - 对应 Node.js 版本的 util/config.json
use std::collections::HashMap;
use std::sync::LazyLock;

pub const API_DOMAIN: &str = "https://interface.music.163.com";
pub const DOMAIN: &str = "https://music.163.com";
pub const ENCRYPT: bool = true;
pub const ENCRYPT_RESPONSE: bool = false;
pub const CHECK_TOKEN: &str = "9ca17ae2e6ffcda170e2e6ee8af14fbabdb988f225b3868eb2c15a879b9a83d274a790ac8ff54a97b889d5d42af0feaec3b92af58cff99c470a7eafd88f75e839a9ea7c14e909da883e83fb692a3abdb6b92adee9e";

/// 资源类型映射表
pub static RESOURCE_TYPE_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("0", "R_SO_4_");
    m.insert("1", "R_MV_5_");
    m.insert("2", "A_PL_0_");
    m.insert("3", "R_AL_3_");
    m.insert("4", "A_DJ_1_");
    m.insert("5", "R_VI_62_");
    m.insert("6", "A_EV_2_");
    m.insert("7", "A_DR_14_");
    m
});

/// OS 配置
#[derive(Debug, Clone)]
pub struct OsConfig {
    pub os: &'static str,
    pub appver: &'static str,
    pub osver: &'static str,
    pub channel: &'static str,
}

pub const OS_PC: OsConfig = OsConfig {
    os: "pc",
    appver: "3.1.17.204416",
    osver: "Microsoft-Windows-10-Professional-build-19045-64bit",
    channel: "netease",
};

pub const OS_LINUX: OsConfig = OsConfig {
    os: "linux",
    appver: "1.2.1.0428",
    osver: "Deepin 20.9",
    channel: "netease",
};

pub const OS_ANDROID: OsConfig = OsConfig {
    os: "android",
    appver: "8.20.20.231215173437",
    osver: "14",
    channel: "xiaomi",
};

pub const OS_IPHONE: OsConfig = OsConfig {
    os: "iPhone OS",
    appver: "9.0.90",
    osver: "16.2",
    channel: "distribution",
};

pub fn get_os_config(os_name: &str) -> &'static OsConfig {
    match os_name {
        "linux" => &OS_LINUX,
        "android" => &OS_ANDROID,
        "iphone" => &OS_IPHONE,
        _ => &OS_PC,
    }
}

/// User-Agent 选择
pub fn choose_user_agent(crypto: &str, ua_type: &str) -> &'static str {
    match (crypto, ua_type) {
        ("weapi", _) => {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0"
        }
        ("linuxapi", _) => {
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/60.0.3112.90 Safari/537.36"
        }
        ("api" | "eapi", "android") => {
            "NeteaseMusic/9.1.65.240927161425(9001065);Dalvik/2.1.0 (Linux; U; Android 14; 23013RK75C Build/UKQ1.230804.001)"
        }
        ("api" | "eapi", "iphone") => "NeteaseMusic 9.0.90/5038 (iPhone; iOS 16.2; zh_CN)",
        ("api" | "eapi", _) => {
            "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36 Chrome/91.0.4472.164 NeteaseMusicDesktop/3.0.18.203152"
        }
        _ => "",
    }
}
