use serde::{Serialize, Deserialize};
use tokio::{fs::File, io::AsyncReadExt};


#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Server {
    pub port: u16,
    pub forward: ((u8, u8, u8, u8), u16),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub db: String,
    pub servers: Vec<Server>
}

impl Config {
    pub async fn from_str(js: &str) -> Option<Config> {
        let res: Option<Config> = match serde_json::from_str(js) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                panic!("error {}", e);
            }
        };
        return res;
    }

    pub async fn from_file(filename: &str) -> Option<Config> {
        let f = File::open(filename).await;
        match f {
            Ok(mut file) => {
                let mut c = String::new();
                file.read_to_string(&mut c).await.unwrap();
                Config::from_str(&c).await
            }
            Err(e) => {
                panic!("error {}", e)
            }
        }
    }
}