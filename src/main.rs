mod config;

use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, time::{SystemTime, UNIX_EPOCH}, fmt::Display};

use rusqlite::Connection;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    spawn,
    task::JoinHandle, sync::{mpsc::{channel, Sender}},
};

#[derive(Debug)]
struct TrackRecord {
    session_id: String,
    data_type: String,
    start_time: u64,
    end_time: u64,
    payload: Option<Vec<u8>>
}

impl Display for TrackRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(data) = &self.payload {
            write!(f, 
                "\nsession_id = {sid}\ndata_type = {dt}\nstart_time = {st}\nend_time = {et}\npayload = {p}\n-----------------split-----------------\n", 
                sid = self.session_id, dt = self.data_type, st = self.start_time, et = self.end_time, p = String::from_utf8_lossy(data))
        } else {
            write!(f, 
                "\nsession_id = {sid}\ndata_type = {dt}\nstart_time = {st}\nend_time = {et}\npayload = null\n-----------------split-----------------\n",
                sid = self.session_id, dt = self.data_type, st = self.start_time, et = self.end_time)
        }
        
    }
}

#[tokio::main]
async fn main() {
    simple_logger::init_with_level(log::Level::Info).unwrap();
    let cfg = config::Config::from_file("config.json").await.unwrap();
    let db = cfg.db.clone();
    let mut th = Vec::<JoinHandle<()>>::with_capacity(cfg.servers.len());

    
    let (tx, mut rx) = channel::<TrackRecord>(100);

    for s in cfg.servers {
        let tx = tx.clone();
        th.push(tokio::spawn(async move {
            run(s, tx).await;
        }));
    }
    th.push(spawn(async move {
        let conn = Connection::open(db).unwrap();
        conn.execute("
            CREATE TABLE IF NOT EXISTS track_record (
                session_id	varchar(36),
                data_type	varchar(10),
                start_time	INTEGER,
                end_time	INTEGER,
                payload	BLOB
            )
        ", ()).unwrap();
        conn.execute("
            CREATE INDEX IF NOT EXISTS session_inx ON track_record (
                session_id
            )
        ", ()).unwrap();
        loop {
            let r = rx.recv().await.unwrap();
            conn.execute("insert into track_record (session_id, data_type, start_time, end_time, payload) values (?1, ?2, ?3, ?4, ?5)", (
                &r.session_id,
                &r.data_type,
                &r.start_time,
                &r.end_time,
                &r.payload
            )).unwrap();
            log::info!("track {}", r);
        }
        
    }));
    for t in th {
        t.await.unwrap();
    }
}

async fn run(cfg: config::Server, tx: Sender<TrackRecord>) {
    log::info!("tracking on {}", cfg.port);
    let server = TcpListener::bind(format!("0.0.0.0:{}", cfg.port))
        .await
        .unwrap();
    let ((_a, _b, _c, _d), port) = cfg.forward;
    let dst = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(_a, _c, _c, _d)), port);

    loop {
        let (stream, _) = server.accept().await.unwrap();
        let dst = dst.clone();
        let tx = tx.clone();
        spawn(async move {
            handle(stream, dst, tx).await;
        });
    }
}

async fn handle(mut stream: TcpStream, dst: SocketAddr, tx: Sender<TrackRecord>) {
    let mut dst_stream = TcpStream::connect(dst).await.unwrap();
    let (mut ro, mut wo) = stream.split();
    let (mut rd, mut wd) = dst_stream.split();
    let uid = uuid::Uuid::new_v4().to_string();
    let u1 = uid.clone();

    let s1 = async {
        let mut rcd = TrackRecord{
            session_id: u1,
            data_type: String::from("request"),
            start_time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            end_time: 0,
            payload: None
        };
        let mut payload = Vec::<u8>::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = ro.read(&mut buf).await {
            if n == 0 {
                break;
            }
            wd.write(&buf[0..n]).await.unwrap();
            payload.extend_from_slice(&buf[0..n]);
        }
        rcd.end_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        rcd.payload = Some(payload);
        tx.send(rcd).await.unwrap();
        wd.flush().await.unwrap();
        wd.shutdown().await.unwrap();
    };
    let s2 = async {
        let mut rcd = TrackRecord{
            session_id: uid,
            data_type: String::from("response"),
            start_time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            end_time: 0,
            payload: None
        };
        let mut payload = Vec::<u8>::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = rd.read(&mut buf).await {
            if n == 0 {
                break;
            }
            wo.write(&buf[0..n]).await.unwrap();
            payload.extend_from_slice(&buf[0..n]);
        }
        rcd.end_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        rcd.payload = Some(payload);
        tx.send(rcd).await.unwrap();
        wo.flush().await.unwrap();
        wo.shutdown().await.unwrap();
    };
    tokio::join!(s1, s2);
}
