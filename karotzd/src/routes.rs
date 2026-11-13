//! The rest of the OpenKarotz CGI API, ported from /usr/www/cgi-bin.
//!
//! Each function keeps the old script's URL, parameters, checks, messages and
//! JSON so the old web UI keeps working. Differences, on purpose:
//!  * nothing runs through a shell; parameters are validated;
//!  * slow work (sounds, ear moves) runs in the background, the reply is immediate;
//!  * actions that write to flash are refused while karotzd runs --readonly;
//!  * no temp files on flash (the old scripts wrote Tmp/tmplst for every count);
//!  * RFID action files are built with safe shell quoting (dbus_watcher `eval`s them);
//!  * `tts` no longer echoes the VoiceRSS API key back in its reply.

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::http::{json_str, respond, Request};
use crate::karotz::{
    ctx, ears_job, err, is_sound_id, killall, led_restore, led_set, readonly_block, remove_state, run,
    run_bg, run_file, sleeping, write_state, EARS_DISABLED, SLEEPING, SOUND_PROCS,
};
use crate::util::{b64_decode, b64_encode, disk_usage, md5_hex, random_below, sh_quote, url_encode};

const OK: &str = "{\"return\":\"0\"}";
const ALL_PROCS: &[&str] = &["led", "ears", "madplay", "mpd", "mplayer", "squeezeslave"];
const MADPLAY: &str = "/usr/bin/madplay";
const EARS: &str = "/karotz/bin/ears";

// ------------------------------------------------------------------ helpers

fn data(p: &str) -> PathBuf {
    ctx().data.join(p)
}

fn read_trim(p: impl AsRef<Path>, default: &str) -> String {
    fs::read_to_string(p).map(|s| s.trim().to_owned()).unwrap_or_else(|_| default.to_owned())
}

/// File names in `dir` ending in `ext`, sorted.
fn files_with(dir: &Path, ext: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.ends_with(ext)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn count_recursive(dir: &Path, ext: &str) -> usize {
    fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() { count_recursive(&p, ext) } else { p.to_string_lossy().ends_with(ext) as usize }
                })
                .sum()
        })
        .unwrap_or(0)
}

fn missing(name: &str) -> String {
    err(&format!("Missing mandatory parameter : {name}"))
}

/// Tag ids, cache ids, snapshot names: letters, digits, _ - . only.
fn safe_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 96 && !s.starts_with('.')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

fn ears_disabled() -> bool {
    run_file("ears.disabled").exists()
}

/// Runs `steps` one after another on a background thread (sound, then ears...).
fn job(steps: impl FnOnce() + Send + 'static) {
    let _ = thread::Builder::new().stack_size(64 * 1024).spawn(steps);
}

fn play_fg(file: &str) -> bool {
    run(MADPLAY, &["-q", file])
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// (year, month, day, hour, min, sec) in UTC, for snapshot file names.
fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + (m <= 2) as i64;
    (y, m, d, (rem / 3600) as u32, (rem / 60 % 60) as u32, (rem % 60) as u32)
}

fn text_reply(s: &TcpStream, body: &str) -> io::Result<()> {
    respond(s, 200, "text/plain", body.as_bytes())
}

fn command_text(prog: &str, args: &[&str]) -> String {
    Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
        .unwrap_or_else(|e| format!("{prog}: {e}\n"))
}

// ------------------------------------------------------------------ status & info

fn mac(iface: &str) -> String {
    let m = read_trim(format!("/sys/class/net/{iface}/address"), "");
    if m.is_empty() { "00:00:00:00:00:00".into() } else { m.to_uppercase() }
}

/// /cgi-bin/status[?format=xml]
pub fn status(r: &Request) -> String {
    let version = read_trim("/usr/www/ok.version", "0");
    let led = read_trim(run_file("led.color"), "00FF00");
    let pulse = read_trim(run_file("led.pulse"), "0");
    let sleep = sleeping();
    let sleep_time = if sleep { read_trim(run_file("karotz.time.sleep"), "0") } else { "0".into() };
    let cs = files_with(&data("Tmp"), ".txt").len();
    let nt = files_with(&data("Rfid"), ".rfid").len();
    let nm = count_recursive(&data("Moods/fr"), ".mp3");
    let ns = files_with(&data("Sounds"), ".mp3").len();
    let nst = files_with(&data("Stories"), ".mp3").len();
    let usb = fs::read_to_string("/proc/mounts").map(|m| m.contains("usbkey")).unwrap_or(false);
    let (ufs, ufsp) = if usb { disk_usage("/mnt/usbkey").unwrap_or(("-1".into(), "".into())) } else { ("-1".into(), "".into()) };
    let (kfs, kfsp) = disk_usage("/usr").unwrap_or_default();
    let ed = ears_disabled() as u8;
    let (eth, wlan) = (mac("eth0"), mac("wlan0"));
    if r.q("format").eq_ignore_ascii_case("xml") {
        return format!(
            "<?xml version=\"1.0\" encoding=\"iso-8859-1\"?>\n<OPENKAROTZ>\n\t<VERSION>{version}</VERSION>\n\
             \t<EARS_DISABLED>{ed}</EARS_DISABLED>\n\t<SLEEP>{}</SLEEP>\n\t<SLEEP_TIME>{sleep_time}</SLEEP_TIME>\n\
             \t<LED_COLOR>{led}</LED_COLOR>\n\t<LED_PULSE>{pulse}</LED_PULSE>\n\t<TTS_CACHE_SIZE>{cs}</TTS_CACHE_SIZE>\n\
             \t<ETH_MAC>{eth}</ETH_MAC>\n\t<WLAN_MAC>{wlan}</WLAN_MAC>\n\t<KAROTZ_USED_SPACE>{kfsp}</KAROTZ_USED_SPACE>\n\
             \t<USB_USED_SPACE>{ufsp}</USB_USED_SPACE>\n\t<RETURN>0</RETURN>\n</OPENKAROTZ>\n",
            sleep as u8
        );
    }
    format!(
        "{{\"version\":\"{version}\",\"ears_disabled\":\"{ed}\",\"sleep\":\"{}\",\"sleep_time\":\"{sleep_time}\",\
         \"led_color\":\"{led}\",\"led_pulse\":\"{pulse}\",\"tts_cache_size\":\"{cs}\",\"usb_free_space\":\"{ufs}\",\
         \"karotz_free_space\":\"{kfs}\",\"eth_mac\":\"{eth}\",\"wlan_mac\":\"{wlan}\",\"nb_tags\":\"{nt}\",\
         \"nb_moods\":\"{nm}\",\"nb_sounds\":\"{ns}\",\"nb_stories\":\"{nst}\",\"karotz_percent_used_space\":\"{kfsp}\",\
         \"usb_percent_used_space\":\"{ufsp}\",\"data_dir\":{}}}",
        sleep as u8,
        json_str(&ctx().data.to_string_lossy())
    )
}

/// /cgi-bin/get_version
pub fn get_version(_r: &Request) -> String {
    let setup = fs::read_to_string("/usr/www/cgi-bin/setup.inc").unwrap_or_default();
    let v = setup
        .lines()
        .find_map(|l| l.trim().strip_prefix("CNF_VERSION="))
        .map(|v| v.trim_matches('"').to_owned())
        .unwrap_or_else(|| "unknown".into());
    format!(
        "{{\"version\":{},\"rootfs_version\":{},\"patch_version\":{},\"return\":\"0\"}}",
        json_str(&v),
        json_str(&read_trim("/karotz/etc/rootfs_version", "Unknown")),
        json_str(&read_trim("/www/ok_patch", "N/A"))
    )
}

/// /cgi-bin/get_free_space
pub fn get_free_space(_r: &Request) -> String {
    let usb = fs::read_to_string("/proc/mounts").map(|m| m.contains("usbkey")).unwrap_or(false);
    let ufsp = if usb { disk_usage("/mnt/usbkey").map(|d| d.1).unwrap_or("-1".into()) } else { "-1".into() };
    let kfsp = disk_usage("/usr").map(|d| d.1).unwrap_or_default();
    format!("{{\"karotz_percent_used_space\":\"{kfsp}\",\"usb_percent_used_space\":\"{ufsp}\"}}")
}

// ------------------------------------------------------------------ sleep, wake, ears, power

/// /cgi-bin/sleep
pub fn sleep(_r: &Request) -> String {
    if sleeping() {
        return err("Unable to perform action, rabbit is already sleeping.");
    }
    if let Some(e) = readonly_block("put the rabbit to sleep") {
        return e;
    }
    killall(ALL_PROCS);
    write_state(&run_file("karotz.sleep"), "1\n");
    write_state(&run_file("karotz.time.sleep"), &format!("{}\n", now_secs()));
    let ears_on = !ears_disabled();
    job(move || {
        if ears_on {
            run(EARS, &[]);
        }
        led_set("00FF66", "00FF00", true, "700", true);
        if ears_on {
            run(EARS, &["5", "50", "5", "50"]);
        }
        led_set("000000", "000000", false, "700", true);
    });
    OK.into()
}

/// /cgi-bin/wakeup[?silent=1]
pub fn wakeup(r: &Request) -> String {
    if let Some(e) = readonly_block("wake the rabbit") {
        return e;
    }
    killall(ALL_PROCS);
    let silent = r.q("silent") == "1";
    remove_state(&run_file("karotz.sleep"));
    remove_state(&run_file("karotz.time.sleep"));
    let ears_on = !ears_disabled();
    job(move || {
        led_set("00FF00", "000000", true, "700", true);
        if ears_on {
            run(EARS, &[]);
        }
        if !silent {
            play_fg("/usr/karotz/res/sounds/karotz_allume.mp3");
        }
    });
    format!("{{\"return\":\"0\",\"silent\":\"{}\"}}", silent as u8)
}

/// /cgi-bin/ears_random[?noreset=1]
pub fn ears_random(r: &Request) -> String {
    if ears_disabled() {
        return err(EARS_DISABLED);
    }
    let (l, rt) = (random_below(15), random_below(15));
    let mut steps = Vec::new();
    if r.q("noreset") != "1" {
        steps.push(vec![]);
    }
    steps.push(vec![l.to_string(), "100".into(), rt.to_string(), "100".into()]);
    ears_job(steps);
    format!("{{\"left\":\"{l}\",\"right\":\"{rt}\",\"return\":\"0\"}}")
}

/// /cgi-bin/ears_mode?disable=1|0
pub fn ears_mode(r: &Request) -> String {
    if r.q("disable") == "1" {
        if let Some(e) = readonly_block("disable the ears") {
            return e;
        }
        write_state(&run_file("ears.disabled"), "1\n");
        return "{\"return\":\"0\",\"disabled\":\"1\"}".into();
    }
    if ears_disabled() {
        if let Some(e) = readonly_block("enable the ears") {
            return e;
        }
        remove_state(&run_file("ears.disabled"));
    }
    ears_job(vec![vec![]]);
    "{\"return\":\"0\",\"disabled\":\"0\"}".into()
}

/// /cgi-bin/reboot  (reboots 10 s later, like the old script)
pub fn reboot(_r: &Request) -> String {
    killall(ALL_PROCS);
    if run_bg("/sbin/reboot", &["-d", "10"]) { OK.into() } else { err("Could not start reboot.") }
}

/// /cgi-bin/setclock  (NTP sync in the background; the rabbit needs internet for it)
pub fn setclock(_r: &Request) -> String {
    if run_bg("ntpd", &["-n", "-q", "-p", "pool.ntp.org"]) { OK.into() } else { err("Could not start ntpd.") }
}

/// /cgi-bin/health_check_config?enable=1|0
pub fn health_check_config(r: &Request) -> String {
    let en = r.q("enable");
    if en.is_empty() {
        return missing("enable");
    }
    if let Some(e) = readonly_block("change the network health check") {
        return e;
    }
    if en == "1" {
        write_state(&run_file("karotz.check"), "1\n");
        "{\"return\":\"0\",\"msg\":\"Network check enabled\"}".into()
    } else {
        remove_state(&run_file("karotz.check"));
        "{\"return\":\"0\",\"msg\":\"Network check disabled\"}".into()
    }
}

/// /cgi-bin/check_health: if enabled, ping the gateway; reboot when it's unreachable.
pub fn check_health(_r: &Request) -> String {
    if !run_file("karotz.check").exists() || sleeping() {
        return "{\"return\":\"0\",\"network\":\"check disabled\"}".into();
    }
    // Default route: the line in /proc/net/route whose destination is 00000000.
    let gw = fs::read_to_string("/proc/net/route").ok().and_then(|t| {
        t.lines().skip(1).find_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            (c.get(1) == Some(&"00000000")).then(|| u32::from_str_radix(c.get(2)?, 16).ok()).flatten()
        })
    });
    let Some(gw) = gw else { return err("No default gateway.") };
    let ip = format!("{}.{}.{}.{}", gw & 255, gw >> 8 & 255, gw >> 16 & 255, gw >> 24);
    if run("ping", &["-c", "3", "-q", &ip]) {
        led_restore();
        format!("{{\"return\":\"0\",\"network\":\"ok\",\"gateway\":\"{ip}\"}}")
    } else {
        led_set("FF0000", "0000FF", true, "100", false);
        run_bg("/sbin/reboot", &[]);
        format!("{{\"return\":\"1\",\"network\":\"failed\",\"gateway\":\"{ip}\",\"msg\":\"Rebooting.\"}}")
    }
}

// ------------------------------------------------------------------ sound apps

/// /cgi-bin/sound_control?cmd=pause|quit|volume 50|...  (mplayer slave commands)
pub fn sound_control(r: &Request) -> String {
    if sleeping() {
        return err(SLEEPING);
    }
    let cmd = r.q("cmd").trim();
    if cmd.is_empty() {
        return err("No command specified.");
    }
    // `run` would make mplayer execute a shell command, so it isn't allowed.
    let allowed = ["seek", "quit", "pause", "loop", "volume", "mute", "loadfile", "loadlist"];
    let word = cmd.split_whitespace().next().unwrap_or("");
    if !allowed.contains(&word) || cmd.contains(['\n', '\r']) {
        return err("Unknown command.");
    }
    if !crate::karotz::is_running(&["mplayer"]) {
        return err("No sound currently playing.");
    }
    use std::os::unix::fs::OpenOptionsExt;
    let w = fs::OpenOptions::new()
        .write(true)
        .custom_flags(0o4000) // O_NONBLOCK: never hang if nobody reads the FIFO
        .open("/tmp/mplayer-control")
        .and_then(|mut f| f.write_all(format!("{cmd}\n").as_bytes()));
    match w {
        Ok(_) => format!("{{\"return\":\"0\",\"cmd\":{}}}", json_str(cmd)),
        Err(e) => err(&format!("Could not send command: {e}")),
    }
}

/// /cgi-bin/stories?id=NAME
pub fn stories(r: &Request) -> String {
    if sleeping() {
        return err(SLEEPING);
    }
    let id = r.q("id");
    if id.is_empty() {
        return err("No story to tell.");
    }
    let f = data("Stories").join(format!("{id}.mp3"));
    if !is_sound_id(id) || !f.is_file() {
        return err(&format!("Unable to find sound : {id}"));
    }
    killall(ALL_PROCS);
    let f = f.to_string_lossy().into_owned();
    if run_bg(MADPLAY, &["-q", &f]) { OK.into() } else { err("Could not start madplay.") }
}

/// /cgi-bin/apps/clock[?hour=H]: a random recording for the hour, then ears reset.
pub fn apps_clock(r: &Request) -> String {
    let dir = data("Apps/Clock/Data");
    if !dir.is_dir() {
        return err("Clock application data missing.");
    }
    if sleeping() {
        return err(SLEEPING);
    }
    let hour: u32 = if r.q("hour").is_empty() {
        civil(now_secs()).3
    } else {
        match r.q("hour").parse() {
            Ok(h) if h < 24 => h,
            _ => return err("hour must be 0-23."),
        }
    };
    let hdir = dir.join(hour.to_string());
    let n = files_with(&hdir, ".mp3").len() as u32;
    if n == 0 {
        return err("No sound for this time.");
    }
    let f = hdir.join(format!("{}.mp3", random_below(n) + 1));
    if !f.is_file() {
        return err("No sound for this time.");
    }
    killall(SOUND_PROCS);
    let f = f.to_string_lossy().into_owned();
    let ears_on = !ears_disabled();
    job(move || {
        play_fg(&f);
        if ears_on {
            run(EARS, &[]);
        }
    });
    format!("{{\"return\":\"0\",\"hour\":\"{hour}\"}}")
}

/// /cgi-bin/apps/moods[?id=N]: a mood recording (random if no id).
pub fn apps_moods(r: &Request) -> String {
    let id = if r.q("id").is_empty() { random_below(305).to_string() } else { r.q("id").to_owned() };
    if id.parse::<u32>().is_err() {
        return err("Moods not found");
    }
    let f = data("Moods/fr").join(format!("{id}.mp3"));
    if !f.is_file() {
        return err("Moods not found");
    }
    if sleeping() {
        return err(SLEEPING);
    }
    killall(&["led", "madplay", "mpd", "mplayer", "squeezeslave"]);
    let f = f.to_string_lossy().into_owned();
    let ears_on = !ears_disabled();
    job(move || {
        if ears_on {
            run(EARS, &[]);
        }
        play_fg(&f);
    });
    format!("{{\"moods\":\"{id}\",\"return\":\"0\"}}")
}

pub fn moods_obsolete(_r: &Request) -> String {
    err("Obsolete, Moods is now an Application. Use /cgi-bin/apps/moods now.")
}

// ------------------------------------------------------------------ lists & cache

pub fn sound_list(_r: &Request) -> String {
    let ids: Vec<String> = files_with(&data("Sounds"), ".mp3")
        .iter()
        .map(|n| format!("{{\"id\":{}}}", json_str(n.trim_end_matches(".mp3"))))
        .collect();
    format!("{{\"sounds\":[{}],\"return\":\"0\"}}", ids.join(","))
}

pub fn voice_list(_r: &Request) -> String {
    "{\"voices\":[{\"id\":\"heather\",\"lang\":\"us\"},{\"id\":\"ryan\",\"lang\":\"us\"}],\"return\":\"0\"}".into()
}

pub fn apps_list(_r: &Request) -> String {
    "{\"apps\":[{\"id\":\"moods\",\"file\":\"apps.moods.install\"},{\"id\":\"funny clock\",\"file\":\"apps.clock.install\"},\
     {\"id\":\"story-Berceuse\",\"file\":\"apps.story1.install\"},{\"id\":\"story-Contes\",\"file\":\"apps.story2.install\"},\
     {\"id\":\"story-Histoires\",\"file\":\"apps.story3.install\"}],\"return\":\"0\"}"
        .into()
}

pub fn moods_list(_r: &Request) -> String {
    fs::read_to_string(data("Moods/fr/moods.json")).unwrap_or_else(|_| err("Unable to found mood list"))
}

pub fn radios_list(_r: &Request) -> String {
    fs::read_to_string(data("Apps/Streams/radios.json")).unwrap_or_else(|_| err("Unable to found radios list"))
}

pub fn stories_list(_r: &Request) -> String {
    if !data("Apps/Story/Data").is_dir() {
        return err("Stories directory missing");
    }
    fs::read_to_string(data("Apps/Story/story.json")).unwrap_or_else(|_| "{\"streams\":[],\"return\":\"0\"}".into())
}

/// /cgi-bin/display_cache: spoken texts cached in Tmp/ (id, text, voice).
pub fn display_cache(_r: &Request) -> String {
    let tmp = data("Tmp");
    let items: Vec<String> = files_with(&tmp, ".txt")
        .iter()
        .map(|n| {
            let id = n.trim_end_matches(".txt");
            let text = read_trim(tmp.join(n), "").replace('"', " ");
            let voice = read_trim(tmp.join(format!("{id}.voice")), "unknown");
            format!("{{\"id\":{},\"text\":{},\"voice\":{}}}", json_str(id), json_str(text.trim()), json_str(&voice))
        })
        .collect();
    format!("{{\"cache\":[{}],\"return\":\"0\"}}", items.join(","))
}

/// /cgi-bin/clear_cache[?id=ID]
pub fn clear_cache(r: &Request) -> String {
    if let Some(e) = readonly_block("clear the speech cache") {
        return e;
    }
    let tmp = data("Tmp");
    let id = r.q("id");
    if id.is_empty() {
        for n in files_with(&tmp, "") {
            let _ = fs::remove_file(tmp.join(n));
        }
        return "{\"return\":\"0\",\"msg\":\"Cache cleared\"}".into();
    }
    if !safe_name(id) || !tmp.join(format!("{id}.mp3")).exists() {
        return err("Unable to find file in cache");
    }
    for n in files_with(&tmp, "").iter().filter(|n| n.starts_with(&format!("{id}."))) {
        let _ = fs::remove_file(tmp.join(n));
    }
    format!("{{\"return\":\"0\",\"msg\":\"File cleared\",\"id\":{}}}", json_str(id))
}

// ------------------------------------------------------------------ camera

static CAMERA: Mutex<()> = Mutex::new(());
const PICTURE: &str = "/tmp/picture.jpg";

/// Asks the webcam daemon for a picture (it writes /tmp/picture.jpg, in RAM).
fn take_picture() -> Result<Vec<u8>, String> {
    let _cam = CAMERA.lock().unwrap_or_else(|p| p.into_inner());
    let _ = fs::remove_file(PICTURE);
    let iface = "com.mindscape.karotz.KarotzInterface";
    let args = [crate::dbus::Arg::S("1"), crate::dbus::Arg::S("file"), crate::dbus::Arg::I(50)];
    if let Err(e) = crate::dbus::call("com.mindscape.karotz.Webcam", "/com/mindscape/karotz/Webcam", iface, "takeSnapshot", &args) {
        return Err(format!("Webcam daemon: {e:?}"));
    }
    fs::read(PICTURE).map_err(|_| "Unable to take snapshot.".to_owned())
}

/// After a picture: the old scripts restore the LED (the daemon flashes it) and
/// play the shutter sound unless silent=1.
fn after_picture(silent: bool) {
    if !sleeping() {
        led_restore();
        if !silent {
            run_bg(MADPLAY, &["-q", "/usr/karotz/res/sounds/webcam.mp3"]);
        }
    }
}

/// /cgi-bin/snapshot_view[?silent=1]: takes a picture and returns the JPEG (RAM only).
pub fn snapshot_view(r: &Request, s: &TcpStream) -> io::Result<()> {
    killall(&["led"]);
    let pic = take_picture();
    after_picture(r.q("silent") == "1");
    let _ = fs::remove_file(PICTURE);
    match pic {
        Ok(jpg) => respond(s, 200, "image/jpeg", &jpg),
        Err(e) => respond(s, 503, "text/plain", format!("{}\n", err(&e)).as_bytes()),
    }
}

/// /cgi-bin/snapshot[?silent=1]: takes a picture and saves it (plus a thumbnail) in Snapshots/.
pub fn snapshot(r: &Request) -> String {
    let dir = data("Snapshots");
    if !dir.is_dir() {
        return err("Snapshot directory not found.");
    }
    if let Some(e) = readonly_block("save a snapshot") {
        return e;
    }
    killall(&["led"]);
    let pic = take_picture();
    after_picture(r.q("silent") == "1");
    let jpg = match pic {
        Ok(j) => j,
        Err(e) => return err(&e),
    };
    let (y, mo, d, h, mi, se) = civil(now_secs());
    let name = format!("snapshot_{y:04}_{mo:02}_{d:02}_{h:02}_{mi:02}_{se:02}");
    let (jp, tp) = (dir.join(format!("{name}.jpg")), dir.join(format!("{name}.thumb.gif")));
    if fs::write(&jp, &jpg).is_err() {
        return err("Unable to save snapshot.");
    }
    let _ = fs::remove_file(PICTURE);
    run("/usr/bin/arm-linux-djpeg", &["-gif", "-scale", "1/4", "-outfile", &tp.to_string_lossy(), &jp.to_string_lossy()]);
    format!("{{\"filename\":\"{name}.jpg\",\"thumb\":\"{name}.thumb.gif\",\"return\":\"0\"}}")
}

pub fn snapshot_list(_r: &Request) -> String {
    let dir = data("Snapshots");
    if !dir.is_dir() {
        return err("Snapshots directory missing");
    }
    let ids: Vec<String> = files_with(&dir, ".jpg").iter().map(|n| format!("{{\"id\":{}}}", json_str(n))).collect();
    format!("{{\"snapshots\":[{}],\"return\":\"0\"}}", ids.join(","))
}

/// /cgi-bin/snapshot_get?filename=NAME.jpg (also serves the .thumb.gif files)
pub fn snapshot_get(r: &Request, s: &TcpStream) -> io::Result<()> {
    let name = r.q("filename");
    let ctype = if name.ends_with(".gif") { "image/gif" } else { "image/jpeg" };
    match fs::read(data("Snapshots").join(name)) {
        Ok(b) if safe_name(name) => respond(s, 200, ctype, &b),
        _ => respond(s, 404, "text/plain", b"snapshot not found\n"),
    }
}

pub fn clear_snapshots(_r: &Request) -> String {
    if let Some(e) = readonly_block("delete the snapshots") {
        return e;
    }
    let dir = data("Snapshots");
    for n in files_with(&dir, "") {
        let _ = fs::remove_file(dir.join(n));
    }
    OK.into()
}

/// /cgi-bin/webcam: live MJPEG (1 frame/s) straight from /dev/camera via ffmpeg,
/// as the old Python script did. One viewer at a time; ffmpeg stops when they leave.
pub fn webcam(_r: &Request, mut s: &TcpStream) -> io::Result<()> {
    let Ok(_cam) = CAMERA.try_lock() else {
        return respond(s, 503, "text/plain", b"camera busy\n");
    };
    let mut child = Command::new("ffmpeg")
        .args(["-f", "rawvideo", "-pix_fmt", "rgb565", "-r", "1", "-s", "640x480", "-i", "/dev/camera",
               "-f", "mpjpeg", "-vcodec", "mjpeg", "-qscale", "1", "-r", "1", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    s.write_all(b"HTTP/1.0 200 OK\r\nServer: karotzd\r\nContent-Type: multipart/x-mixed-replace; boundary=ffserver\r\n\
                  Cache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\n\r\n")?;
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 16 * 1024];
    let r = loop {
        match out.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                if let Err(e) = s.write_all(&buf[..n]) {
                    break Err(e);
                }
            }
            Err(e) => break Err(e),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    r
}

// ------------------------------------------------------------------ text to speech

/// The VoiceRSS API key: $VOICERSS_KEY, else the first line of <datadir>/voicerss.key.
/// No key is built in: get a free one at voicerss.org.
fn voicerss_key() -> Option<String> {
    let from_env = std::env::var("VOICERSS_KEY").ok();
    let key = from_env.or_else(|| fs::read_to_string(data("voicerss.key")).ok())?;
    let key = key.lines().next().unwrap_or("").trim().to_owned();
    (!key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(key)
}

fn tts_lang(voice: &str) -> &'static str {
    match voice {
        "lucy" | "graham" => "en-gb",
        "heather" | "ryan" => "en-us",
        "andreas" | "julia" => "de-de",
        "chiara" | "vittorio" => "it-it",
        "femke" | "daan" | "jasmijn" | "zoe" => "nl-nl",
        _ => "fr-fr",
    }
}

/// /cgi-bin/tts?voice=V&text=T: fetches speech from VoiceRSS (cached in /tmp) and plays it.
pub fn tts(r: &Request) -> String {
    let (voice, text) = (r.q("voice").to_lowercase(), r.q("text"));
    if voice.is_empty() || text.is_empty() {
        return "{\"return\":false,\"id\":\"\",\"played\":false,\"cache\":false,\"voice\":\"\"}".into();
    }
    let id = md5_hex(format!("{voice}{text}").as_bytes());
    let mp3 = format!("/tmp/{id}.mp3");
    let cached = Path::new(&mp3).is_file();
    if !cached {
        let Some(key) = voicerss_key() else {
            return format!(
                "{{\"return\":false,\"id\":\"{id}\",\"played\":false,\"cache\":false,\"voice\":{},\
                 \"msg\":\"No VoiceRSS key: set VOICERSS_KEY or put one in {}\"}}",
                json_str(&voice),
                data("voicerss.key").display()
            );
        };
        let url = format!(
            "http://api.voicerss.org?key={key}&hl={}&f=48khz_16bit_mono&src={}",
            tts_lang(&voice),
            url_encode(text)
        );
        let ok = run("wget", &["-q", "-T", "20", "-O", &mp3, &url]);
        if !ok || fs::metadata(&mp3).map(|m| m.len() == 0).unwrap_or(true) {
            let _ = fs::remove_file(&mp3);
        }
    }
    let have = Path::new(&mp3).is_file();
    let played = have && !sleeping() && run_bg(MADPLAY, &["-A6", "-q", &mp3]);
    format!(
        "{{\"return\":{},\"id\":\"{id}\",\"played\":{played},\"cache\":{cached},\"voice\":{}}}",
        have, json_str(&voice)
    )
}

// ------------------------------------------------------------------ RFID

fn rfid(p: &str) -> PathBuf {
    data("Rfid").join(p)
}

fn tag_known(tag: &str) -> bool {
    safe_name(tag) && rfid(&format!("{tag}.rfid")).exists()
}

fn tag_field(tag: &str, ext: &str, default: &str) -> String {
    read_trim(rfid(&format!("{tag}.{ext}")), default)
}

fn type_name(t: &str) -> &'static str {
    match t { "1" => "FLATANOZ", "2" => "NANOZTAG", "3" => "ZSTAMPS", "15" => "KEYRING", _ => "UNKNOWN" }
}

fn color_name(c: &str) -> &'static str {
    match c {
        "1" => "RED", "2" => "BLUE", "3" => "GREEN", "4" => "YELLOW", "5" => "PINK", "6" => "BLACK",
        "7" => "GREY", "8" => "ORANGE", "9" => "PURPLE", "10" => "WHITE", "51" => "BROWN", _ => "UNKNOWN",
    }
}

fn cmd_name(c: &str) -> &'static str {
    match c {
        "1" => "VERA", "2" => "EEDOMUS", "3" => "URL", "4" => "ZIBASE", "5" => "KAROTZ", "6" => "CALAOS",
        "7" => "PLUGIN", _ => "Unknown",
    }
}

fn tag_ids() -> Vec<String> {
    files_with(&data("Rfid"), ".rfid").iter().map(|n| n.trim_end_matches(".rfid").to_owned()).collect()
}

fn tag_json(tag: &str, ext: bool) -> String {
    let (t, c, cmd) = (tag_field(tag, "type", "-1"), tag_field(tag, "color", "-1"), tag_field(tag, "cmd", "0"));
    let mut j = format!(
        "{{\"id\":{},\"name\":{},\"type\":{},\"color\":{},\"cmd\":{},\"var\":{}",
        json_str(tag), json_str(&tag_field(tag, "name", "")), json_str(&t), json_str(&c), json_str(&cmd),
        json_str(&tag_field(tag, "var", "-1"))
    );
    if ext {
        j += &format!(
            ",\"type_name\":\"{}\",\"color_name\":\"{}\",\"cmd_name\":\"{}\",\"has_action\":{}",
            type_name(&t), color_name(&c), cmd_name(&cmd), rfid(&format!("{tag}.action")).exists()
        );
    }
    j + "}"
}

pub fn rfid_list(_r: &Request) -> String {
    let tags: Vec<String> = tag_ids().iter().map(|t| tag_json(t, false)).collect();
    format!("{{\"tags\":[{}],\"return\":\"0\"}}", tags.join(","))
}

pub fn rfid_list_ext(_r: &Request) -> String {
    if !data("Rfid").is_dir() {
        return err("Rfid directory missing");
    }
    let tags: Vec<String> = tag_ids().iter().map(|t| tag_json(t, true)).collect();
    format!("{{\"tags\":[{}],\"return\":\"0\"}}", tags.join(","))
}

pub fn rfid_info(r: &Request) -> String {
    let tag = r.q("tag");
    if tag.is_empty() {
        return err("Missing mandatory parameters.");
    }
    if !tag_known(tag) {
        return err("Tag ID not found.");
    }
    let j = tag_json(tag, true);
    format!("{},\"return\":\"0\"}}", &j[..j.len() - 1])
}

/// Common start of the tag-writing routes: tag present, known, not read-only.
fn tag_for_write<'a>(r: &'a Request, what: &str) -> Result<&'a str, String> {
    let tag = r.q("tag");
    if tag.is_empty() {
        return Err(missing("tag"));
    }
    if !tag_known(tag) {
        return Err(err("Tag ID not found."));
    }
    if let Some(e) = readonly_block(what) {
        return Err(e);
    }
    Ok(tag)
}

pub fn rfid_rename(r: &Request) -> String {
    let tag = match tag_for_write(r, "rename a tag") { Ok(t) => t, Err(e) => return e };
    let name = r.q("name");
    if name.is_empty() {
        let _ = fs::remove_file(rfid(&format!("{tag}.name")));
    } else {
        let _ = fs::write(rfid(&format!("{tag}.name")), format!("{}\n", name.replace(['\n', '\r'], " ")));
    }
    let color = if r.q("color").parse::<u32>().is_ok() { r.q("color") } else { "31" };
    let _ = fs::write(rfid(&format!("{tag}.color")), format!("{color}\n"));
    OK.into()
}

fn remove_tag_files(tag: &str, exts: &[&str]) {
    for e in exts {
        let _ = fs::remove_file(rfid(&format!("{tag}.{e}")));
    }
}

pub fn rfid_delete(r: &Request) -> String {
    if !data("Rfid").is_dir() {
        return err("Rfid directory not found.");
    }
    let tag = match tag_for_write(r, "delete a tag") { Ok(t) => t, Err(e) => return e };
    for n in files_with(&data("Rfid"), "").iter().filter(|n| n.starts_with(&format!("{tag}."))) {
        let _ = fs::remove_file(rfid(n));
    }
    format!("{{\"return\":\"0\",\"tag\":{}}}", json_str(tag))
}

pub fn rfid_unassign(r: &Request) -> String {
    let tag = match tag_for_write(r, "unassign a tag") { Ok(t) => t, Err(e) => return e };
    remove_tag_files(tag, &["cmd", "var", "action", "name"]);
    format!("{{\"return\":\"0\",\"tag\":{}}}", json_str(tag))
}

pub fn clear_rfid(_r: &Request) -> String {
    if let Some(e) = readonly_block("delete all tags") {
        return e;
    }
    for n in files_with(&data("Rfid"), "") {
        let _ = fs::remove_file(rfid(&n));
    }
    OK.into()
}

/// /cgi-bin/rfid_start_record: the next tags shown to the rabbit get registered
/// (dbus_watcher does the registering while Run/rfid.record exists).
pub fn rfid_start_record(_r: &Request) -> String {
    if !data("Run").is_dir() {
        return err("Run directory not found.");
    }
    if let Some(e) = readonly_block("start tag recording") {
        return e;
    }
    killall(ALL_PROCS);
    write_state(&run_file("rfid.record"), "1\n");
    let voice = data("Voice/Rfid_Start_Record.mp3").to_string_lossy().into_owned();
    let ears_on = !ears_disabled();
    job(move || {
        play_fg(&voice);
        if ears_on {
            run(EARS, &[]);
        }
        led_set("AABB00", "FF0000", true, "700", false);
    });
    OK.into()
}

pub fn rfid_stop_record(_r: &Request) -> String {
    if !data("Run").is_dir() {
        return err("Run directory not found.");
    }
    if let Some(e) = readonly_block("stop tag recording") {
        return e;
    }
    killall(ALL_PROCS);
    remove_state(&run_file("rfid.record"));
    let voice = data("Voice/Rfid_Stop_record.mp3").to_string_lossy().into_owned();
    job(move || {
        play_fg(&voice);
        led_restore();
    });
    OK.into()
}

fn is_http_url(u: &str) -> bool {
    (u.starts_with("http://") || u.starts_with("https://")) && !u.bytes().any(|b| b.is_ascii_control())
}

/// `url` plus the optional base64 `param` query string, as BuildUrl did.
fn full_url(r: &Request) -> Result<String, String> {
    let url = r.q("url");
    let param = if r.q("param").is_empty() {
        String::new()
    } else {
        b64_decode(r.q("param")).map(|b| String::from_utf8_lossy(&b).trim().to_owned()).ok_or_else(|| err("param must be base64."))?
    };
    let u = if param.is_empty() { url.to_owned() } else { format!("{url}?{param}") };
    if is_http_url(&u) { Ok(u) } else { Err(err("url must start with http:// or https://")) }
}

/// Writes a tag's action file: base64 of a curl command line that dbus_watcher
/// will `eval` when the tag is shown. Every value is single-quoted.
fn write_action(tag: &str, cmd_type: &str, curl: &str, name: Option<&str>) {
    remove_tag_files(tag, &["cmd", "var", "action", "name"]);
    let _ = fs::write(rfid(&format!("{tag}.cmd")), format!("{cmd_type}\n"));
    let _ = fs::write(rfid(&format!("{tag}.var")), "-1\n");
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        let _ = fs::write(rfid(&format!("{tag}.name")), format!("{}\n", n.replace(['\n', '\r'], " ")));
    }
    let _ = fs::write(rfid(&format!("{tag}.action")), format!("{}\n", b64_encode(format!("{curl}\n").as_bytes())));
}

/// /cgi-bin/rfid_assign_url?tag=T&url=U[&param=BASE64][&name=N][&user=&password=]
pub fn rfid_assign_url(r: &Request) -> String {
    if r.q("url").is_empty() {
        return missing("url");
    }
    let url = match full_url(r) { Ok(u) => u, Err(e) => return e };
    let tag = match tag_for_write(r, "assign a tag") { Ok(t) => t, Err(e) => return e };
    let mut curl = "curl -g --connect-timeout 30 -o /tmp/curlout -s -L -k".to_owned();
    if !r.q("user").is_empty() {
        curl += &format!(" --user {}", sh_quote(&format!("{}:{}", r.q("user"), r.q("password"))));
    }
    curl += &format!(" {}", sh_quote(&url));
    write_action(tag, "3", &curl, Some(r.q("name")));
    OK.into()
}

/// /cgi-bin/rfid_assign_karotz?tag=T&url=BASE64(url): the tag calls one of the rabbit's own URLs.
pub fn rfid_assign_karotz(r: &Request) -> String {
    if r.q("url").is_empty() {
        return missing("url");
    }
    let url = match b64_decode(r.q("url")) {
        Some(b) => String::from_utf8_lossy(&b).trim().to_owned(),
        None => return err("url must be base64."),
    };
    if !is_http_url(&url) {
        return err("url must start with http:// or https://");
    }
    let tag = match tag_for_write(r, "assign a tag") { Ok(t) => t, Err(e) => return e };
    write_action(tag, "5", &format!("curl -L -k -s -o /tmp/curlout {}", sh_quote(&url)), None);
    OK.into()
}

/// /cgi-bin/rfid_test_url?url=U[&param=BASE64][&user=&password=]: calls the URL now.
pub fn rfid_test_url(r: &Request) -> String {
    if r.q("url").is_empty() {
        return missing("url");
    }
    let url = match full_url(r) { Ok(u) => u, Err(e) => return e };
    let mut args = vec!["-g", "--connect-timeout", "30", "-s", "-L", "-k"];
    let auth = format!("{}:{}", r.q("user"), r.q("password"));
    if !r.q("user").is_empty() {
        args.extend(["--user", &auth]);
    }
    args.push(&url);
    match Command::new("curl").args(&args).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => {
            let body = String::from_utf8_lossy(&o.stdout);
            let body = if body.trim().is_empty() { "No output received from command".into() } else { body.chars().take(2000).collect::<String>() };
            format!("{{\"return\":\"0\",\"msg\":{}}}", json_str(&body))
        }
        Ok(o) => err(&format!("curl error {}", o.status.code().unwrap_or(-1))),
        Err(e) => err(&format!("curl: {e}")),
    }
}

pub fn tag_list_obsolete(_r: &Request) -> String {
    err("Obsolete, use rfid_list instead.")
}

// ------------------------------------------------------------------ tools

pub fn tools_ps(_r: &Request, s: &TcpStream) -> io::Result<()> {
    text_reply(s, &command_text("ps", &[]))
}

pub fn tools_net(_r: &Request, s: &TcpStream) -> io::Result<()> {
    text_reply(s, &(command_text("ifconfig", &[]) + "\n" + &command_text("route", &["-n"])))
}

pub fn tools_ls(r: &Request, s: &TcpStream) -> io::Result<()> {
    let src = r.q("src");
    if !src.starts_with('/') || !Path::new(src).is_dir() {
        return text_reply(s, "Not found\n");
    }
    text_reply(s, &command_text("ls", &["-la", src]))
}

pub fn tools_log(_r: &Request, s: &TcpStream) -> io::Result<()> {
    let log = fs::read_to_string("/var/log/messages").unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    text_reply(s, &(lines[lines.len().saturating_sub(200)..].join("\n") + "\n"))
}

/// /var/log is in RAM, so this is safe in read-only mode too.
pub fn tools_clearlog(_r: &Request) -> String {
    for n in files_with(Path::new("/var/log"), "").iter().filter(|n| n.starts_with("messages")) {
        let _ = fs::remove_file(Path::new("/var/log").join(n));
    }
    let _ = fs::write("/var/log/messages", "");
    OK.into()
}

pub fn tools_controllog(r: &Request) -> String {
    match r.q("cmd") {
        "start" => {
            if let Some(e) = readonly_block("enable the system log") {
                return e;
            }
            killall(&["syslogd"]);
            let _ = fs::write("/etc/conf/enable_syslogd", "");
            run_bg("/sbin/syslogd", &["-s", "50", "-b", "2"]);
            OK.into()
        }
        "stop" => {
            if let Some(e) = readonly_block("disable the system log") {
                return e;
            }
            let _ = fs::remove_file("/etc/conf/enable_syslogd");
            killall(&["syslogd"]);
            OK.into()
        }
        _ => err("Nothing to do"),
    }
}

pub fn cmdlist(_r: &Request, s: &TcpStream) -> io::Result<()> {
    text_reply(s, "seek\nquit\npause\nloop\nvolume\nmute\nloadfile\nloadlist\n")
}

// ------------------------------------------------------------------ blocked

/// Old scripts karotzd refuses to pass through, with the reason.
pub fn blocked(path: &str) -> Option<&'static str> {
    Some(match path {
        "/cgi-bin/cmd" => "runs any shell command from the URL as root",
        "/cgi-bin/get_file" | "/cgi-bin/get_list" => "downloads a file into any path on the rabbit",
        "/cgi-bin/save_snapshot" => "uploads photos to a hard-coded FTP server with a hard-coded password",
        "/cgi-bin/api_test" | "/cgi-bin/create_ip_sound" | "/cgi-bin/test.py" => "a leftover developer test script",
        _ => return None,
    })
}
