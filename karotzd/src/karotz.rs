//! Native versions of the OpenKarotz CGI routes for LEDs, ears and sound.
//!
//! Same URLs, parameters, checks and JSON replies as the bash scripts in
//! /usr/www/cgi-bin, so the existing web UI works unchanged. Differences:
//!  * Programs run from argument lists, never through a shell.
//!  * Parameters are validated (colors are 6 hex digits, numbers are numbers).
//!  * Slow work (ear homing, playing a clip) is queued and the reply is sent at once.
//!  * With --readonly, the LED "memory" files in Run/ are never written.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::http::Request;

pub struct Ctx {
    pub data: PathBuf,
    pub readonly: bool,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static EARS: Mutex<()> = Mutex::new(());

pub fn init(ctx: Ctx) {
    let _ = CTX.set(ctx);
}

pub(crate) fn ctx() -> &'static Ctx {
    CTX.get().expect("karotz::init not called")
}

pub(crate) fn run_file(name: &str) -> PathBuf {
    ctx().data.join("Run").join(name)
}

pub(crate) fn sleeping() -> bool {
    run_file("karotz.sleep").exists()
}

pub(crate) fn err(msg: &str) -> String {
    format!("{{\"return\":\"1\",\"msg\":{}}}", crate::http::json_str(msg))
}

pub(crate) const SLEEPING: &str = "Unable to perform action, rabbit is sleeping.";
pub(crate) const EARS_DISABLED: &str = "Unable to perform action, ears disabled.";
pub(crate) const SOUND_PROCS: &[&str] = &["madplay", "mpd", "mplayer", "squeezeslave"];

// ------------------------------------------------------------------ validation

pub fn is_hex6(s: &str) -> bool {
    s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn is_sound_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with('.')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

pub fn is_stream_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() < 2048
        && !s.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

// ------------------------------------------------------------------ processes

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
}

/// PIDs whose process name (from /proc/<pid>/stat; 2.6.28 has no /proc/<pid>/comm)
/// is one of `names`.
fn pids_named(names: &[&str]) -> Vec<i32> {
    let me = std::process::id() as i32;
    let Ok(dir) = fs::read_dir("/proc") else { return Vec::new() };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&pid| pid != me)
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| {
                    let (a, b) = (s.find('(')?, s.rfind(')')?);
                    Some(names.contains(&&s[a + 1..b]))
                })
                .unwrap_or(false)
        })
        .collect()
}

/// True if any process with one of these names is running.
pub(crate) fn is_running(names: &[&str]) -> bool {
    !pids_named(names).is_empty()
}

/// Like `killall name...`, then waits (up to 1 s) until they are gone.
pub fn killall(names: &[&str]) {
    let pids = pids_named(names);
    for &pid in &pids {
        unsafe { kill(pid, 15) };
    }
    let t = Instant::now();
    while !pids.is_empty() && !pids_named(names).is_empty() && t.elapsed() < Duration::from_secs(1) {
        thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
}

/// Runs a program to completion; true if it exited 0.
pub(crate) fn run(prog: &str, args: &[&str]) -> bool {
    quiet(&mut Command::new(prog)).args(args).status().map(|s| s.success()).unwrap_or(false)
}

/// Starts a program and returns at once; a tiny thread reaps it when it exits.
pub(crate) fn run_bg(prog: &str, args: &[&str]) -> bool {
    match quiet(&mut Command::new(prog)).args(args).spawn() {
        Ok(mut child) => {
            let reaped = thread::Builder::new().stack_size(32 * 1024).spawn(move || {
                let _ = child.wait();
            });
            reaped.is_ok()
        }
        Err(_) => false,
    }
}

/// Runs jobs on the ears one at a time, in order, without blocking the reply.
pub(crate) fn ears_job(steps: Vec<Vec<String>>) {
    let _ = thread::Builder::new().stack_size(32 * 1024).spawn(move || {
        let _guard = EARS.lock().unwrap_or_else(|p| p.into_inner());
        for args in steps {
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            run("/karotz/bin/ears", &a);
        }
    });
}

pub(crate) fn uuid() -> String {
    fs::read_to_string("/proc/sys/kernel/random/uuid").map(|s| s.trim().to_owned()).unwrap_or_default()
}

/// Calls the LED daemon over our own D-Bus connection (a few ms). If the bus
/// can't be reached, falls back to spawning dbus-send like the old scripts.
pub(crate) fn dbus_led(method: &str, extra: &[String]) -> bool {
    let id = uuid();
    let mut args = vec![crate::dbus::Arg::S(&id)];
    for e in extra {
        if let Some(s) = e.strip_prefix("string:") {
            args.push(crate::dbus::Arg::S(s));
        } else if let Some(i) = e.strip_prefix("int32:").and_then(|i| i.parse().ok()) {
            args.push(crate::dbus::Arg::I(i));
        }
    }
    let iface = "com.mindscape.karotz.KarotzInterface";
    match crate::dbus::call("com.mindscape.karotz.Led", "/com/mindscape/karotz/Led", iface, method, &args) {
        Ok(_) => true,
        Err(crate::dbus::CallError::Remote(e)) => {
            eprintln!("LED {method}: {e}");
            false
        }
        Err(crate::dbus::CallError::Transport(e)) => {
            eprintln!("D-Bus unavailable ({e}); using dbus-send");
            dbus_send_led(method, extra)
        }
    }
}

fn dbus_send_led(method: &str, extra: &[String]) -> bool {
    let mut args = vec![
        "--system".to_owned(),
        "--dest=com.mindscape.karotz.Led".to_owned(),
        "/com/mindscape/karotz/Led".to_owned(),
        format!("com.mindscape.karotz.KarotzInterface.{method}"),
        format!("string:{}", uuid()),
    ];
    args.extend_from_slice(extra);
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    run("dbus-send", &a)
}

pub(crate) fn write_state(path: &Path, contents: &str) {
    if ctx().readonly {
        eprintln!("readonly: not writing {}", path.display());
    } else {
        let _ = fs::write(path, contents);
    }
}

pub(crate) fn remove_state(path: &Path) {
    if ctx().readonly {
        eprintln!("readonly: not removing {}", path.display());
    } else {
        let _ = fs::remove_file(path);
    }
}

/// The old `Leds` helper: light or pulse the LED, and (if `memory`) remember it
/// in Run/led.color and Run/led.pulse, unless karotzd runs --readonly.
pub(crate) fn led_set(color: &str, color2: &str, pulse: bool, speed: &str, memory: bool) -> bool {
    let ok = if pulse {
        dbus_led("pulse", &[
            format!("string:{color}"), format!("string:{color2}"),
            format!("int32:{speed}"), "int32:-1".to_owned(),
        ])
    } else {
        dbus_led("light", &[format!("string:{color}")])
    };
    if memory {
        if pulse {
            write_state(&run_file("led.pulse"), "1
");
        } else if run_file("led.pulse").exists() {
            remove_state(&run_file("led.pulse"));
        }
        write_state(&run_file("led.color"), &format!("{color}
"));
    }
    ok
}

/// The old `LedsRestore`: re-apply the remembered color/pulse (writes nothing).
pub(crate) fn led_restore() -> bool {
    let color = fs::read_to_string(run_file("led.color")).map(|s| s.trim().to_owned()).unwrap_or_default();
    let color = if is_hex6(&color) { color } else { "00FF00".to_owned() };
    led_set(&color, "000000", run_file("led.pulse").exists(), "700", false)
}

/// Refuses an action that would write to flash while karotzd runs --readonly.
pub(crate) fn readonly_block(what: &str) -> Option<String> {
    ctx().readonly.then(|| err(&format!("karotzd is running read-only, so it won't {what} (that writes to flash).")))
}

// ------------------------------------------------------------------ routes

/// /cgi-bin/leds?color=RRGGBB&pulse=0|1&color2=RRGGBB&speed=ms&nomemory=0|1
pub fn leds(r: &Request) -> String {
    if !ctx().data.join("Run").exists() {
        return err("Run directory not found.");
    }
    if sleeping() {
        return err(SLEEPING);
    }
    if !r.q("pulse").is_empty() && !r.q("blink").is_empty() {
        return err("You cannot use BLINK and PULSE parameters at the same time.");
    }
    let nomemory = r.q("nomemory") == "1";
    let speed = if r.q("speed").is_empty() { "700" } else { r.q("speed") };
    let color2 = if r.q("color2").is_empty() { "000000" } else { r.q("color2") };
    let color = if r.q("color").is_empty() {
        fs::read_to_string(run_file("led.color")).map(|s| s.trim().to_owned()).unwrap_or_else(|_| "00FF00".into())
    } else {
        r.q("color").to_owned()
    };
    let pulse = r.q("pulse") == "1";
    if !is_hex6(&color) || !is_hex6(color2) {
        return err("Colors must be 6 hex digits, like FF0000.");
    }
    if !speed.parse::<u32>().map(|v| (1..=60000).contains(&v)).unwrap_or(false) {
        return err("Speed must be a number of milliseconds (1-60000).");
    }
    killall(&["led"]); // the old script stops any running `led` animation first

    let ok = led_set(&color, color2, pulse, speed, !nomemory);
    if !ok {
        return err("dbus-send to the LED daemon failed.");
    }
    format!(
        "{{\"color\":\"{color}\",\"secondary_color\":\"{color2}\",\"pulse\":\"{}\",\"no_memory\":\"{}\",\
         \"speed\":\"{speed}\",\"return\":\"0\"}}",
        pulse as u8, nomemory as u8
    )
}

/// /cgi-bin/ears?left=N&right=N&noreset=0|1
pub fn ears(r: &Request) -> String {
    if run_file("ears.disabled").exists() {
        return err(EARS_DISABLED);
    }
    if sleeping() {
        return err(SLEEPING);
    }
    let (l, rt) = (r.q("left"), r.q("right"));
    if l.is_empty() || rt.is_empty() {
        return err("Missing mandatory parameters.");
    }
    let (Ok(li), Ok(ri)) = (l.parse::<i32>(), rt.parse::<i32>()) else {
        return err("left and right must be whole numbers.");
    };
    let mut steps = Vec::new();
    if r.q("noreset") != "1" {
        steps.push(vec![]); // `ears` with no arguments homes both ears
    }
    // Same argument order as ears.inc: right, speed, left, speed.
    steps.push(vec![ri.to_string(), "100".into(), li.to_string(), "100".into()]);
    ears_job(steps);
    format!("{{\"left\":\"{li}\",\"right\":\"{ri}\",\"return\":\"0\"}}")
}

/// /cgi-bin/ears_reset  (the old script also stops every LED, ear and sound process)
pub fn ears_reset(_r: &Request) -> String {
    killall(&["led", "ears", "madplay", "mpd", "mplayer", "squeezeslave"]);
    if run_file("ears.disabled").exists() {
        return err(EARS_DISABLED);
    }
    if sleeping() {
        return err(SLEEPING);
    }
    ears_job(vec![vec![]]);
    "{\"return\":\"0\"}".to_owned()
}

/// /cgi-bin/sound?id=NAME  or  /cgi-bin/sound?url=http://...
pub fn sound(r: &Request) -> String {
    if sleeping() {
        return err(SLEEPING);
    }
    let (id, url) = (r.q("id"), r.q("url"));
    if !id.is_empty() && !url.is_empty() {
        return err("You cannot use ID and URL parameters at the same time.");
    }
    if id.is_empty() && url.is_empty() {
        return err("No sound to play.");
    }
    if !id.is_empty() {
        let file = ctx().data.join("Sounds").join(format!("{id}.mp3"));
        if !is_sound_id(id) || !file.is_file() {
            return err(&format!("Unable to find sound : {}", if is_sound_id(id) { id } else { "(invalid name)" }));
        }
        killall(SOUND_PROCS);
        let f = file.to_string_lossy();
        return if run_bg("/usr/bin/madplay", &["-q", &f]) { "{\"return\":\"0\"}".into() } else { err("Could not start madplay.") };
    }
    if !is_stream_url(url) {
        return err("url must start with http:// or https://");
    }
    killall(SOUND_PROCS);
    let fifo = "/tmp/mplayer-control";
    if !Path::new(fifo).exists() {
        unsafe { mkfifo(c"/tmp/mplayer-control".as_ptr(), 0o644) };
    }
    let input = format!("file={fifo}");
    if run_bg("/usr/bin/mplayer", &["-quiet", "-slave", "-input", &input, url]) {
        "{\"return\":\"0\"}".into()
    } else {
        err("Could not start mplayer.")
    }
}
