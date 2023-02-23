mod config;

use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, time::{SystemTime, UNIX_EPOCH}, fmt::Display, sync::Arc};

use rusqlite::Connection;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader, AsyncBufReadExt},
    net::{TcpListener, TcpStream},
    spawn,
    task::JoinHandle, sync::{mpsc::{channel, Sender}, Mutex},
};

#[derive(Debug)]
struct TrackRecord {
    session_id: String,
    data_type: String,
    start_time: u64,
    end_time: u64,
    header: Option<String>,
    payload: Option<Vec<u8>>
}

impl Display for TrackRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(data) = &self.header {
            write!(f, 
                "\nsession_id = {sid}\ndata_type = {dt}\nstart_time = {st}\nend_time = {et}\nheader = {p}\n-----------------split-----------------\n", 
                sid = self.session_id, dt = self.data_type, st = self.start_time, et = self.end_time, p = data)
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
                header TEXT,
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
            conn.execute("insert into track_record (session_id, data_type, start_time, end_time, header, payload) values (?1, ?2, ?3, ?4, ?5, ?6)", (
                &r.session_id,
                &r.data_type,
                &r.start_time,
                &r.end_time,
                &r.header,
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
    let session_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let s1 = async {
        let mut ro = BufReader::new(&mut ro);

        let mut ever_read = Vec::<u8>::with_capacity(1024);
        loop {
            let mut must_close = false;
            // 一个循环读一次http请求/响应
            let mut body_len = 0usize;
            let mut chunked = false;
            let mut _gzip = false;
            let mut body_start = false;
            let mut header = Vec::<u8>::with_capacity(1024);
            let uid = uuid::Uuid::new_v4().to_string();
            session_id.lock().await.replace(uid.clone());
            let mut rcd = TrackRecord{
                session_id: uid,
                data_type: String::from("request"),
                start_time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                end_time: 0,
                header: None,
                payload: None
            };
            let mut body = Vec::<u8>::with_capacity(1024);
            loop {
                ever_read.clear();
                if let Err(_) = ro.read_until(b'\n', &mut ever_read).await {
                    must_close = true;
                    break;
                }
                if ever_read.len() == 0 {
                    must_close = true;
                    break;
                }
                
                wd.write_all(&ever_read).await.unwrap();
                if ever_read.starts_with(b"Content-Length: ") {
                    let klen = &ever_read[16..ever_read.len() - 2];
                    body_len = String::from_utf8_lossy(klen).parse::<usize>().unwrap();
                }
                if ever_read.starts_with(b"Transfer-Encoding: chunked") {
                    chunked = true;
                }
                if ever_read.starts_with(b"Content-Encoding: gzip") {
                    _gzip = true;
                }
                if body_start && chunked {
                    body.extend(&ever_read);
                    if body.ends_with(b"0\r\n\r\n") {
                        // chunked 结束
                        break;
                    }
                } else {
                    header.extend(&ever_read);
                }
                
                if ever_read == b"\r\n" {
                    if body_len != 0 {
                        let mut body2 = Vec::<u8>::with_capacity(body_len);
                        unsafe {
                            body2.set_len(body_len);
                        }
                        ro.read_exact(&mut body2).await.unwrap();
                        wd.write_all(&body2).await.unwrap();
                        body.clear();
                        body.extend(&body2);
                        // 这里读完
                        break;
                    } else if chunked {
                        body_start = true;
                    } else {
                        break;
                    }
                }
            }
            rcd.end_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let header = String::from_utf8_lossy(&header);
            rcd.header = Some(header.to_string());
            rcd.payload = Some(body);
            tx.send(rcd).await.unwrap();
            if must_close {
                break;
            }
        }
        wd.flush().await.unwrap();
        wd.shutdown().await.unwrap();
    };
    let s2 = session_id.clone();
    let s2 = async {
        let mut rd = BufReader::new(&mut rd);

        let mut ever_read = Vec::<u8>::with_capacity(1024);
        
        loop {
            // 一个循环读一次http请求/响应
            let mut must_close = false;
            let mut body_len = 0usize;
            let mut body_start = false;
            let mut _gzip = false;
            let mut chunked = false;
            let mut header = Vec::<u8>::with_capacity(1024);
            let sid = s2.lock().await.take();
            let mut rcd = TrackRecord{
                session_id: sid.unwrap(),
                data_type: String::from("response"),
                start_time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                end_time: 0,
                header: None,
                payload: None
            };
            let mut body = Vec::<u8>::with_capacity(1024);
            loop {
                ever_read.clear();
                if let Err(_) = rd.read_until(b'\n', &mut ever_read).await {
                    must_close = true;
                    break;
                }
                if ever_read.len() == 0 {
                    must_close = true;
                    break;
                }
                wo.write_all(&ever_read).await.unwrap();
                if ever_read.starts_with(b"Content-Length: ") {
                    let klen = &ever_read[16..ever_read.len() - 2];
                    body_len = String::from_utf8_lossy(klen).parse::<usize>().unwrap();
                }
                if ever_read.starts_with(b"Transfer-Encoding: chunked") {
                    chunked = true;
                }
                if ever_read.starts_with(b"Content-Encoding: gzip") {
                    _gzip = true;
                }
                if body_start && chunked {
                    body.extend(&ever_read);
                    if body.ends_with(b"0\r\n\r\n") {
                        // chunked 结束
                        break;
                    }
                } else {
                    header.extend(&ever_read);
                }
                
                if ever_read == b"\r\n" {
                    if body_len != 0 {
                        let mut body2 = Vec::<u8>::with_capacity(body_len);
                        unsafe {
                            body2.set_len(body_len);
                        }
                        rd.read_exact(&mut body2).await.unwrap();
                        wo.write_all(&body2).await.unwrap();
                        body.clear();
                        body.extend(&body2);
                        // 这里读完
                        break;
                    } else if chunked {
                        body_start = true;
                    } else {
                        break;
                    }
                }
            }
            rcd.end_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let header = String::from_utf8_lossy(&header);
            rcd.header = Some(header.to_string());
            rcd.payload = Some(body);
            tx.send(rcd).await.unwrap();
            if must_close {
                break;
            }
        }
        wo.flush().await.unwrap();
        wo.shutdown().await.unwrap();
    };
    tokio::join!(s1, s2);
}
