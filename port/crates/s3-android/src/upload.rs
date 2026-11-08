//! Built-in web upload server for the game files (launcher "Game files" page, docs/porting/android.md
//! "Launcher"). A minimal HTTP/1.1 server on std::net: the user opens `http://<phone ip>:<port>/?k=<token>` in a
//! PC browser on the same network, picks the Settlers III folder (`<input webkitdirectory>` or drag and drop) and
//! the page uploads every file with one `POST /upload` per file. Files go below the game dir only (path
//! sanitizing, size limit); the server runs only while the user has it switched on.
//!
//! Routes (all need `k=<token>`, otherwise 403):
//! - `GET /` the upload page (HTML + JS, no external resources);
//! - `GET /status` JSON: missing required parts, files/bytes received;
//! - `GET /stat?path=P` JSON `{"size":N}` (-1 = not there), so the page can skip files already on the phone;
//! - `POST /upload?path=P` body = file contents (Content-Length required), written to `<root>/P` via a
//!   `.upload-part` temp file + rename.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// First port tried (then the next ones, then any free port).
pub const DEFAULT_PORT: u16 = 8080;
/// Largest single file accepted (the biggest Settlers III files are a few hundred MB).
pub const MAX_FILE_BYTES: u64 = 4 << 30;
/// Deepest accepted path (components).
const MAX_DEPTH: usize = 12;
const MAX_HEAD: usize = 16 * 1024;

/// What the launcher shows while the server runs.
#[derive(Debug, Clone, Default)]
pub struct UploadStatus {
    /// Files completely written, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// File being received now (relative path), bytes so far and its size.
    pub current: Option<(String, u64, u64)>,
    /// Last error (bad path, write failed, ...).
    pub error: Option<String>,
    /// Address of the last client.
    pub client: Option<String>,
    /// Bumped on every change.
    pub seq: u64,
}

pub struct UploadServer {
    pub port: u16,
    pub token: String,
    pub root: PathBuf,
    pub status: Arc<Mutex<UploadStatus>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Returns the required parts still missing below a root (the launcher passes `frontend::missing_parts`).
pub type MissingFn = fn(&Path) -> Vec<&'static str>;

impl UploadServer {
    /// Listen on all interfaces, port [`DEFAULT_PORT`] (or the next free one), writing below `root`.
    pub fn start(root: PathBuf, missing: MissingFn) -> std::io::Result<UploadServer> {
        let listener = (DEFAULT_PORT..DEFAULT_PORT + 10)
            .find_map(|p| TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).ok())
            .map_or_else(|| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)), Ok)?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let token = new_token();
        let status = Arc::new(Mutex::new(UploadStatus::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(Ctx { root: root.clone(), token: token.clone(), status: status.clone(), missing });
        let st = stop.clone();
        let thread = std::thread::Builder::new().name("s3-upload".into()).spawn(move || {
            while !st.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((s, addr)) => {
                        let c = ctx.clone();
                        let _ = std::thread::Builder::new().name("s3-upload-conn".into()).spawn(move || {
                            let _ = s.set_nonblocking(false);
                            let _ = s.set_read_timeout(Some(Duration::from_secs(60)));
                            c.status.lock().unwrap().client = Some(addr.ip().to_string());
                            if let Err(e) = handle(&c, s) {
                                log::warn!("upload: connection from {addr}: {e}");
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                    Err(e) => {
                        log::warn!("upload: accept: {e}");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
            log::info!("upload: server stopped");
        })?;
        log::info!("upload: server on port {port}, writing into {}", root.display());
        Ok(UploadServer { port, token, root, status, stop, thread: Some(thread) })
    }

    /// The address to open, for the given phone IP.
    pub fn url(&self, ip: &str) -> String {
        format!("http://{ip}:{}/?k={}", self.port, self.token)
    }

    pub fn snapshot(&self) -> UploadStatus {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for UploadServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The phone's address in the local network: a private IPv4 address (Wi-Fi / hotspot) if there is one.
pub fn local_ip() -> Option<Ipv4Addr> {
    let private = |ip: &Ipv4Addr| ip.is_private();
    let primary = s3_net::lan::primary_ipv4();
    if let Some(p) = primary.filter(private) {
        return Some(p);
    }
    s3_net::lan::interfaces().into_iter().map(|(ip, _)| ip).find(private).or(primary)
}

/// 6 lowercase letters / digits from the clock and address-space randomness (only keeps other devices in the
/// network from writing files by accident or by guessing the port).
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut v = h.finish();
    const A: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    (0..6)
        .map(|_| {
            let c = A[(v % A.len() as u64) as usize] as char;
            v /= A.len() as u64;
            c
        })
        .collect()
}

struct Ctx {
    root: PathBuf,
    token: String,
    status: Arc<Mutex<UploadStatus>>,
    missing: MissingFn,
}

impl Ctx {
    fn update(&self, f: impl FnOnce(&mut UploadStatus)) {
        let mut s = self.status.lock().unwrap();
        f(&mut s);
        s.seq += 1;
    }
}

struct Request {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    content_length: Option<u64>,
    expect_continue: bool,
}

impl Request {
    fn q(&self, k: &str) -> Option<&str> {
        self.query.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
}

fn read_head(r: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut it = line.split_whitespace();
    let (method, target) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("/").to_string());
    let (mut content_length, mut expect_continue, mut total) = (None, false, line.len());
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h)?;
        total += n;
        if n == 0 || total > MAX_HEAD {
            return Err(std::io::Error::other("bad request head"));
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "content-length" {
                content_length = v.parse().ok();
            } else if k == "expect" && v.eq_ignore_ascii_case("100-continue") {
                expect_continue = true;
            }
        }
    }
    let (path, qs) = target.split_once('?').unwrap_or((&target, ""));
    let query = qs
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect();
    Ok(Some(Request { method, path: path.to_string(), query, content_length, expect_continue }))
}

/// `%XX` decoding (UTF-8; `+` stays `+`, `encodeURIComponent` encodes spaces as `%20`).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Checks an uploaded relative path (`/` or `\` separated): no `..`, `.`, empty parts, drive letters, control
/// characters or reserved names; at most [`MAX_DEPTH`] parts. Returns the parts.
pub fn sanitize(path: &str) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("empty path".into());
    }
    if parts.len() > MAX_DEPTH {
        return Err(format!("path too deep: {path}"));
    }
    for p in &parts {
        if *p == "." || *p == ".." || p.contains(':') || p.chars().any(|c| c.is_control()) || p.len() > 200 {
            return Err(format!("bad path: {path}"));
        }
        if p.ends_with(".upload-part") {
            return Err(format!("bad name: {path}"));
        }
    }
    Ok(parts.iter().map(|s| s.to_string()).collect())
}

/// `root` joined with `parts`, reusing existing entries whose names differ only in case (the game looks files
/// up case-insensitively too, so `gfx/` and `GFX/` must not end up as two folders).
pub fn resolve_ci(root: &Path, parts: &[String]) -> PathBuf {
    let mut cur = root.to_path_buf();
    for p in parts {
        let exact = cur.join(p);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let found = std::fs::read_dir(&cur).ok().and_then(|rd| {
            rd.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(p)).map(|e| e.path())
        });
        cur = found.unwrap_or(exact);
    }
    cur
}

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Error",
    };
    write!(
        s,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)?;
    s.flush()
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn status_json(c: &Ctx) -> String {
    let missing = (c.missing)(&c.root);
    let s = c.status.lock().unwrap().clone();
    format!(
        "{{\"missing\":[{}],\"files\":{},\"bytes\":{},\"error\":{}}}",
        missing.iter().map(|m| json_str(m)).collect::<Vec<_>>().join(","),
        s.files,
        s.bytes,
        s.error.as_deref().map_or("null".into(), json_str)
    )
}

fn handle(c: &Ctx, stream: TcpStream) -> std::io::Result<()> {
    let mut w = stream.try_clone()?;
    let mut r = BufReader::with_capacity(256 * 1024, stream);
    let Some(req) = read_head(&mut r)? else { return Ok(()) };
    if req.path == "/favicon.ico" {
        return respond(&mut w, 404, "text/plain", b"");
    }
    if req.q("k") != Some(c.token.as_str()) {
        let msg = "<!doctype html><meta charset=utf-8><title>Settlers 3 Mobile</title><body style=\"font:18px sans-serif\">\
                   Please open the full address shown on the phone (it ends with <b>?k=...</b>).</body>";
        return respond(&mut w, 403, "text/html; charset=utf-8", msg.as_bytes());
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => respond(&mut w, 200, "text/html; charset=utf-8", PAGE.as_bytes()),
        ("GET", "/status") => respond(&mut w, 200, "application/json", status_json(c).as_bytes()),
        ("GET", "/stat") => {
            let size = sanitize(req.q("path").unwrap_or(""))
                .ok()
                .and_then(|p| std::fs::metadata(resolve_ci(&c.root, &p)).ok())
                .filter(|m| m.is_file())
                .map_or(-1, |m| m.len() as i64);
            respond(&mut w, 200, "application/json", format!("{{\"size\":{size}}}").as_bytes())
        }
        ("POST", "/upload") => {
            let rel = req.q("path").unwrap_or("").to_string();
            let parts = match sanitize(&rel) {
                Ok(p) => p,
                Err(e) => {
                    c.update(|s| s.error = Some(e.clone()));
                    return respond(&mut w, 400, "text/plain", e.as_bytes());
                }
            };
            let Some(len) = req.content_length else {
                return respond(&mut w, 400, "text/plain", b"Content-Length required");
            };
            if len > MAX_FILE_BYTES {
                let e = format!("{rel}: file too large ({len} bytes)");
                c.update(|s| s.error = Some(e.clone()));
                return respond(&mut w, 413, "text/plain", e.as_bytes());
            }
            if req.expect_continue {
                w.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            }
            match receive(c, &mut r, &parts, &rel, len) {
                Ok(()) => {
                    c.update(|s| {
                        s.files += 1;
                        s.bytes += len;
                        s.current = None;
                    });
                    respond(&mut w, 200, "application/json", status_json(c).as_bytes())
                }
                Err(e) => {
                    let e = format!("{rel}: {e}");
                    log::warn!("upload: {e}");
                    c.update(|s| {
                        s.error = Some(e.clone());
                        s.current = None;
                    });
                    respond(&mut w, 400, "text/plain", e.as_bytes())
                }
            }
        }
        _ => respond(&mut w, 404, "text/plain", b"not found"),
    }
}

fn receive(c: &Ctx, r: &mut impl Read, parts: &[String], rel: &str, len: u64) -> std::io::Result<()> {
    let dest = resolve_ci(&c.root, parts);
    if !dest.starts_with(&c.root) {
        return Err(std::io::Error::other("outside the game folder"));
    }
    if dest.is_dir() {
        return Err(std::io::Error::other("a folder with this name exists"));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_file_name(format!("{}.upload-part", dest.file_name().unwrap_or_default().to_string_lossy()));
    c.update(|s| s.current = Some((rel.to_string(), 0, len)));
    let res = (|| {
        let mut f = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
        let mut buf = vec![0u8; 256 * 1024];
        let mut left = len;
        let mut since = 0u64;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = r.read(&mut buf[..want])?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed during upload"));
            }
            f.write_all(&buf[..n])?;
            left -= n as u64;
            since += n as u64;
            if since >= 4 << 20 {
                since = 0;
                c.update(|s| s.current = Some((rel.to_string(), len - left, len)));
            }
        }
        f.flush()?;
        drop(f);
        std::fs::rename(&tmp, &dest)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// The upload page. Plain HTML/JS, no external resources.
const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Settlers 3 Mobile - game files</title>
<style>
body{font:16px system-ui,sans-serif;background:#0e1422;color:#e8ecf4;margin:0;padding:24px;max-width:860px}
h1{font-size:24px;margin:0 0 4px}.dim{color:#a8b3c8}.card{background:#1a2440;border-radius:10px;padding:16px;margin:16px 0}
#drop{border:2px dashed #5b6f9c;border-radius:10px;padding:28px;text-align:center}#drop.over{background:#24345c}
button,label.btn{background:#2e6cd0;color:#fff;border:0;border-radius:8px;padding:10px 18px;font-size:16px;cursor:pointer;display:inline-block}
button:disabled{background:#4a5266;cursor:default}.ok{color:#6fdc8c}.bad{color:#ff8080}
progress{width:100%;height:18px}#log{white-space:pre-wrap;font:13px monospace;max-height:220px;overflow:auto}
</style></head><body>
<h1>Settlers 3 Mobile</h1><div class="dim">Copy your own Settlers III installation to the phone. No game files come with the app.</div>
<div class="card"><b>On the phone:</b> <span id="st">checking ...</span></div>
<div class="card">
<p>Choose the Settlers III folder (the folder that contains <b>S3.EXE</b>, <b>GFX</b>, <b>SND</b>, <b>Map</b> ...,
e.g. <span class="dim">C:\GOG Games\Settlers 3 Ultimate</span>), or drop it here:</p>
<div id="drop"><label class="btn">Choose folder<input id="dir" type="file" webkitdirectory multiple hidden></label>
<p class="dim">or drag and drop the folder onto this box</p></div>
<p><label><input id="skip" type="checkbox" checked> Skip files that are already on the phone (same size)</label></p>
<p id="sel" class="dim">No folder chosen.</p>
<button id="go" disabled>Upload</button>
<p><progress id="bar" value="0" max="1"></progress></p><div id="now" class="dim"></div>
</div>
<div class="card"><div id="log" class="dim"></div></div>
<script>
const K=new URLSearchParams(location.search).get('k');let files=[];
const $=id=>document.getElementById(id);
function log(t){$('log').textContent+=t+'\n';$('log').scrollTop=1e9}
function mb(n){return (n/1048576).toFixed(1)+' MB'}
async function status(){try{const r=await fetch('/status?k='+K);const j=await r.json();
$('st').innerHTML=j.missing.length?'<span class="bad">still missing: '+j.missing.join(', ')+'</span>':'<span class="ok">all required game files are there. Tap Play on the phone.</span>';}catch(e){$('st').textContent='phone not reachable'}}
function choose(list){
 // Root = the folder that holds S3.EXE (shortest path), else the top folder.
 let root=null;for(const f of list){const p=f.p.split('/');if(p[p.length-1].toLowerCase()=='s3.exe'){const d=p.slice(0,-1).join('/');if(root===null||d.length<root.length)root=d}}
 if(root===null){const tops=new Set(list.map(f=>f.p.split('/')[0]));root=(tops.size==1&&list.every(f=>f.p.includes('/')))?[...tops][0]:''}
 const pre=root?root+'/':'';files=list.filter(f=>f.p.startsWith(pre)).map(f=>({f:f.f,p:f.p.slice(pre.length)}));
 const tot=files.reduce((a,f)=>a+f.f.size,0);
 $('sel').innerHTML=files.length+' files, '+mb(tot)+(root?' from <b>'+root+'</b>':'')+(list.some(f=>f.p.split('/').pop().toLowerCase()=='s3.exe')?'':' <span class="bad">(no S3.EXE in this folder!)</span>');
 $('go').disabled=!files.length}
$('dir').onchange=e=>choose([...e.target.files].map(f=>({f,p:f.webkitRelativePath||f.name})));
const dz=$('drop');dz.ondragover=e=>{e.preventDefault();dz.classList.add('over')};dz.ondragleave=()=>dz.classList.remove('over');
dz.ondrop=async e=>{e.preventDefault();dz.classList.remove('over');const out=[];
 async function walk(en,path){if(en.isFile){const f=await new Promise((a,b)=>en.file(a,b));out.push({f,p:path+en.name})}
  else if(en.isDirectory){const rd=en.createReader();let ents;do{ents=await new Promise((a,b)=>rd.readEntries(a,b));for(const c of ents)await walk(c,path+en.name+'/')}while(ents.length)}}
 for(const it of [...e.dataTransfer.items].map(i=>i.webkitGetAsEntry()))if(it)await walk(it,'');choose(out)};
function put(f,p,onp){return new Promise((ok,bad)=>{const x=new XMLHttpRequest();x.open('POST','/upload?k='+K+'&path='+encodeURIComponent(p));
 x.upload.onprogress=e=>onp(e.loaded);x.onload=()=>x.status==200?ok():bad(new Error(x.responseText||x.status));x.onerror=()=>bad(new Error('network error'));x.send(f)})}
$('go').onclick=async()=>{ $('go').disabled=true;const tot=files.reduce((a,f)=>a+f.f.size,0);let done=0,sent=0,skipped=0,t0=Date.now(),errs=0;$('bar').max=tot||1;
 for(const {f,p} of files){
  if($('skip').checked){try{const j=await (await fetch('/stat?k='+K+'&path='+encodeURIComponent(p))).json();if(j.size==f.size){done+=f.size;skipped++;$('bar').value=done;continue}}catch(e){}}
  try{await put(f,p,l=>{$('bar').value=done+l;const s=(sent+l)/Math.max(1,(Date.now()-t0)/1000);$('now').textContent=p+'  '+mb(done+l)+' / '+mb(tot)+'  ('+mb(s)+'/s)'});sent+=f.size}
  catch(e){errs++;log('FAILED '+p+': '+e.message)}
  done+=f.size;$('bar').value=done}
 log('Done: '+(files.length-skipped-errs)+' uploaded, '+skipped+' skipped, '+errs+' failed.');$('now').textContent='';$('go').disabled=false;status()};
status();setInterval(status,5000);
</script></body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_escapes() {
        assert_eq!(sanitize("GFX/a.gfx").unwrap(), vec!["GFX", "a.gfx"]);
        assert_eq!(sanitize("\\Map\\x.map").unwrap(), vec!["Map", "x.map"]);
        assert!(sanitize("../x").is_err());
        assert!(sanitize("a/./b").is_err());
        assert!(sanitize("C:/x").is_err());
        assert!(sanitize("").is_err());
        assert!(sanitize("a\u{0}b").is_err());
        assert!(sanitize(&"a/".repeat(20)).is_err());
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("GFX%2F1%20a.gfx"), "GFX/1 a.gfx");
        assert_eq!(percent_decode("a+b%"), "a+b%");
        assert_eq!(percent_decode("%C3%BC"), "ü");
    }

    fn no_missing(_: &Path) -> Vec<&'static str> {
        vec!["GFX"]
    }

    #[test]
    fn upload_roundtrip() {
        let dir = std::env::temp_dir().join(format!("s3upload-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("gfx")).unwrap();
        let srv = UploadServer::start(dir.clone(), no_missing).unwrap();
        let send = |req: String, body: &[u8]| {
            let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            s.write_all(body).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let k = srv.token.clone();
        let r = send(format!("POST /upload?k={k}&path=GFX%2F1.gfx HTTP/1.1\r\nContent-Length: 5\r\n\r\n"), b"hello");
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        // Existing `gfx/` is reused (case-insensitive).
        assert_eq!(std::fs::read(dir.join("gfx/1.gfx")).unwrap(), b"hello");
        let r = send(format!("POST /upload?k={k}&path=..%2Fx HTTP/1.1\r\nContent-Length: 1\r\n\r\n"), b"x");
        assert!(r.starts_with("HTTP/1.1 400"), "{r}");
        let r = send("GET /?k=wrong HTTP/1.1\r\n\r\n".into(), b"");
        assert!(r.starts_with("HTTP/1.1 403"), "{r}");
        let r = send(format!("GET /stat?k={k}&path=GFX/1.gfx HTTP/1.1\r\n\r\n"), b"");
        assert!(r.ends_with("{\"size\":5}"), "{r}");
        let r = send(format!("GET /status?k={k} HTTP/1.1\r\n\r\n"), b"");
        assert!(r.contains("\"missing\":[\"GFX\"],\"files\":1,\"bytes\":5"), "{r}");
        let st = srv.snapshot();
        assert_eq!((st.files, st.bytes), (1, 5));
        drop(srv);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
