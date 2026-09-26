// Copyright (C) 2026 Joshua Simmons. GPL-3.0-or-later.
// Native command line interface for the original ponysay artwork and format.
mod render;

use render::{render, RenderOptions};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct Asset {
    group: &'static str,
    name: &'static str,
    canonical: &'static str,
    data: &'static str,
}
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

#[derive(Clone)]
struct Pony {
    name: String,
    canonical: String,
    data: String,
}

#[derive(Default)]
struct Options {
    files: Vec<(String, u8)>,
    quote: bool,
    list: Option<u8>,
    balloons: bool,
    quoters: bool,
    think: bool,
    only: bool,
    compact: bool,
    no_color: bool,
    info: u8,
    tty: Option<bool>,
    wrap: Option<String>,
    balloon: Option<String>,
    roots: Vec<PathBuf>,
    message: Vec<String>,
}

const HELP: &str = r#"ponysay-rust 4.0.0 (Rust)
Usage: ponysay [options] [--] [message ...]
       ponythink [options] [message ...]
       fortune | ponysay

  -f, --pony NAME       Select pony name or a .pony file; repeat to randomize
  +f NAME              Select a pony from the extra collection
  -F NAME              Select from either collection
  -q, --quote [NAME]   Random quotation, optionally from a named pony
  -l, -L, --list        List pony names (includes aliases)
  +l, +L               List extra pony names
  -A, --all            List both collections
      --quoters        List ponies with quotations
  -B, --balloonlist     List balloon styles
  -b, --balloon STYLE   Choose a balloon style or .say/.think file
  -W, --wrap COLUMNS    Wrap message (default 65); 'n' disables, 'i' uses terminal
  -c, --compact        Collapse message whitespace
  -o, --pony-only       Display artwork without a message
      --think          Use thought bubbles (also selected by ponythink)
  -i, --info           Print pony metadata; +i speaks the metadata
  -X / -V              Use 256-color / Linux-console artwork
      --no-color       Remove ANSI sequences (also respects NO_COLOR)
      --data-dir DIR   Search DIR/{ponies,extraponies,quotes,balloons} first
  -v, --version        Print version
  -h, --help           Print this help

All original ponies, aliases, quotes, and balloons are bundled. User assets in
$XDG_DATA_HOME/ponysay or ~/.local/share/ponysay override bundled assets.
Messages are literal text: backslashes, quotes, and dollar signs are never code.
"#;

fn expand_short_cluster(raw: &str) -> Result<Option<Vec<String>>, String> {
    if raw.len() <= 2 || raw.starts_with("--") || raw.starts_with("++") {
        return Ok(None);
    }
    let sign = match raw.as_bytes()[0] {
        b'-' => '-',
        b'+' => '+',
        _ => return Ok(None),
    };
    let mut flags = Vec::new();
    for (index, ch) in raw.char_indices().skip(1) {
        let takes_value = (sign == '-' && matches!(ch, 'f' | 'F' | 'q' | 'b' | 'W'))
            || (sign == '+' && ch == 'f');
        let boolean = if sign == '-' {
            matches!(
                ch,
                'h' | 'v' | 'l' | 'L' | 'A' | 'B' | 'c' | 'o' | 'i' | 'X' | 'V'
            )
        } else {
            matches!(ch, 'h' | 'l' | 'L' | 'A' | 'i')
        };
        if !takes_value && !boolean {
            return Err(format!("unknown option {sign}{ch} in {raw:?}; see --help"));
        }
        flags.push(format!("{sign}{ch}"));
        if takes_value {
            let remaining = &raw[index + ch.len_utf8()..];
            if !remaining.is_empty() {
                flags.push(remaining.to_owned());
            }
            break;
        }
    }
    Ok(Some(flags))
}

fn parse(mut args: Vec<String>) -> Result<Options, String> {
    let mut opts = Options {
        think: env::args_os()
            .next()
            .and_then(|p| {
                PathBuf::from(p).file_stem().map(|s| {
                    let name = s.to_string_lossy();
                    name.ends_with("think") || name.ends_with("think-rust")
                })
            })
            .unwrap_or(false),
        no_color: env::var_os("NO_COLOR").is_some(),
        ..Options::default()
    };
    let mut i = 0;
    while i < args.len() {
        if let Some(flags) = expand_short_cluster(&args[i])? {
            args.splice(i..=i, flags);
        }
        let raw = &args[i];
        let (flag, inline) = if raw.starts_with("--") || raw.starts_with("++") {
            raw.split_once('=')
                .map_or((raw.as_str(), None), |(a, b)| (a, Some(b.to_owned())))
        } else if raw.len() > 2
            && ["-f", "+f", "-F", "-W", "-b", "-q"]
                .iter()
                .any(|prefix| raw.starts_with(prefix))
        {
            (&raw[..2], Some(raw[2..].to_owned()))
        } else {
            (raw.as_str(), None)
        };
        if inline.is_some()
            && (raw.starts_with("--") || raw.starts_with("++"))
            && !matches!(
                flag,
                "--file"
                    | "--pony"
                    | "++file"
                    | "++pony"
                    | "--any-file"
                    | "--anyfile"
                    | "--any-pony"
                    | "--anypony"
                    | "--quote"
                    | "--f"
                    | "--files"
                    | "--ponies"
                    | "++f"
                    | "++files"
                    | "++ponies"
                    | "--F"
                    | "--any-ponies"
                    | "--q"
                    | "--quotes"
                    | "--bubble"
                    | "--balloon"
                    | "--wrap"
                    | "--data-dir"
            )
        {
            return Err(format!("{flag} does not accept a value; see --help"));
        }
        let mut value = || -> Result<String, String> {
            if let Some(ref v) = inline {
                return Ok(v.clone());
            }
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag {
            "--" => { opts.message.extend_from_slice(&args[i+1..]); break; }
            "-h" | "+h" | "--help" | "++help" | "--help-colour" => return Err(HELP.to_owned()),
            "-v" | "--version" => return Err(format!("ponysay-rust {} (Rust)", env!("CARGO_PKG_VERSION"))),
            "-f" | "--file" | "--pony" => opts.files.push((value()?, 0)),
            "+f" | "++file" | "++pony" => opts.files.push((value()?, 1)),
            "-F" | "--any-file" | "--anyfile" | "--any-pony" | "--anypony" => opts.files.push((value()?, 2)),
            "-q" | "--quote" => {
                opts.quote = true;
                if let Some(v) = inline { opts.files.push((v,0)); }
                else if args.get(i+1).is_some_and(|s| !s.starts_with('-') && !s.starts_with('+')) { i += 1; opts.files.push((args[i].clone(),0)); }
            }
            "--f" | "--files" | "--ponies" | "++f" | "++files" | "++ponies" | "--F" | "--any-ponies" | "--q" | "--quotes" => {
                if flag == "--q" || flag == "--quotes" { opts.quote = true; }
                let group = if flag.starts_with("++") {1} else if flag == "--F" || flag == "--any-ponies" {2} else {0};
                if let Some(v) = inline { opts.files.push((v, group)); }
                while args.get(i+1).is_some_and(|s| !s.starts_with('-') && !s.starts_with('+')) { i += 1; opts.files.push((args[i].clone(),group)); }
            }
            "-l" | "-L" | "--list" | "--symlist" | "--altlist" | "--onelist" => opts.list = Some(0),
            "+l" | "+L" | "++list" | "++symlist" | "++altlist" | "++onelist" => opts.list = Some(1),
            "-A" | "+A" | "--all" | "++all" | "--symall" | "--altall" | "--Onelist" => opts.list = Some(2),
            "--quoters" => opts.quoters = true,
            "-B" | "--bubblelist" | "--balloonlist" => opts.balloons = true,
            "-b" | "--bubble" | "--balloon" => opts.balloon = Some(value()?),
            "-W" | "--wrap" => opts.wrap = Some(value()?),
            "-c" | "--compact" | "--compress" => opts.compact = true,
            "-o" | "--pony-only" | "--ponyonly" => opts.only = true,
            "--think" => opts.think = true,
            "--no-color" => opts.no_color = true,
            "-i" | "--info" => opts.info = 1,
            "+i" | "++info" => opts.info = 2,
            "-X" | "--256-colours" | "--256colours" | "--x-colours" => opts.tty = Some(false),
            "-V" | "--tty-colours" | "--ttycolours" | "--vt-colours" => opts.tty = Some(true),
            "--data-dir" => opts.roots.push(PathBuf::from(value()?)),
            _ if raw.starts_with('-') || raw.starts_with('+') => return Err(format!("unknown option {raw:?}; see --help (use -- before literal text beginning with - or +)")),
            _ => { opts.message.extend_from_slice(&args[i..]); break; }
        }
        i += 1;
    }
    Ok(opts)
}

fn roots(explicit: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = explicit.to_vec();
    if let Some(v) = env::var_os("PONYSAY_DATA_DIR") {
        paths.extend(env::split_paths(&v));
    }
    if let Some(v) = env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(v).join("ponysay"));
    }
    if let Some(v) = env::var_os("HOME") {
        paths.push(PathBuf::from(v).join(".local/share/ponysay"));
    }
    paths.push(PathBuf::from("/usr/local/share/ponysay"));
    paths.push(PathBuf::from("/usr/share/ponysay"));
    paths
}

fn group_names(group: u8, tty: bool) -> Vec<&'static str> {
    match (group, tty) {
        (0, false) => vec!["ponies"],
        (1, false) => vec!["extraponies"],
        (0, true) => vec!["ttyponies"],
        (1, true) => vec!["extrattyponies"],
        (_, false) => vec!["ponies", "extraponies"],
        (_, true) => vec!["ttyponies", "extrattyponies"],
    }
}

fn names(groups: &[&str], dirs: &[PathBuf]) -> Vec<String> {
    let mut list: BTreeMap<String, ()> = ASSETS
        .iter()
        .filter(|a| groups.contains(&a.group))
        .map(|a| (a.name.to_owned(), ()))
        .collect();
    for root in dirs {
        for group in groups {
            if let Ok(entries) = fs::read_dir(root.join(group)) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                            if !name.starts_with('.')
                                && (!group.contains("ponies") || name.ends_with(".pony"))
                            {
                                list.insert(name.into(), ());
                            }
                        }
                    }
                }
            }
        }
    }
    list.into_keys().collect()
}

fn load(name: &str, groups: &[&str], dirs: &[PathBuf]) -> Result<(String, String), String> {
    for root in dirs {
        for group in groups {
            let p = root.join(group).join(name);
            if p.is_file() {
                return load_path(&p);
            }
        }
    }
    ASSETS
        .iter()
        .find(|a| groups.contains(&a.group) && a.name == name)
        .map(|a| (a.canonical.into(), a.data.into()))
        .ok_or_else(|| format!("asset {name:?} not found"))
}

fn load_path(path: &Path) -> Result<(String, String), String> {
    let data = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    Ok((
        canonical
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        data,
    ))
}

fn pony(name: &str, group: u8, tty: bool, dirs: &[PathBuf]) -> Result<Pony, String> {
    let (canonical, data) = if Path::new(name).is_file() {
        load_path(Path::new(name))?
    } else {
        let file = if name.ends_with(".pony") {
            name.to_owned()
        } else {
            format!("{name}.pony")
        };
        load(&file, &group_names(group, tty), dirs)
            .map_err(|_| format!("pony {name:?} not found; use -l or -A to list names"))?
    };
    Ok(Pony {
        name: name.trim_end_matches(".pony").to_owned(),
        canonical: canonical.trim_end_matches(".pony").to_owned(),
        data,
    })
}

fn choose<T>(items: &[T]) -> Result<&T, String> {
    if items.is_empty() {
        return Err("no matching assets found".into());
    }
    let mut bytes = [0u8; 8];
    let seed = if fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok()
    {
        u64::from_ne_bytes(bytes)
    } else {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    };
    Ok(&items[(seed % items.len() as u64) as usize])
}

fn quotes_for<'a>(files: &'a [String], name: &str) -> Vec<&'a String> {
    files
        .iter()
        .filter(|f| {
            f.rsplit_once('.')
                .map(|(stem, _)| stem.split('+').any(|part| part == name))
                .unwrap_or(false)
        })
        .collect()
}

fn metadata(data: &str) -> &str {
    data.strip_prefix("$$$")
        .and_then(|s| s.split_once("$$$"))
        .map(|(m, _)| m.trim())
        .unwrap_or("No metadata for this pony.")
}

fn has_quotes(name: &str, group: u8, tty: bool, dirs: &[PathBuf], files: &[String]) -> bool {
    if !quotes_for(files, name.trim_end_matches(".pony")).is_empty() {
        return true;
    }
    pony(name, group, tty, dirs).is_ok_and(|p| !quotes_for(files, &p.canonical).is_empty())
}

fn strip_ansi(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            result.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                if chars.peek() == Some(&'P') {
                    // Linux VT palette sequences are ESC ] P plus seven hex
                    // digits, without the OSC terminator used by xterm.
                    for _ in 0..8 {
                        chars.next();
                    }
                    continue;
                }
                if chars.peek() == Some(&'R') {
                    chars.next();
                    continue;
                }
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('P' | '_' | '^') => {
                while let Some(c) = chars.next() {
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn run(opts: Options) -> Result<String, String> {
    let dirs = roots(&opts.roots);
    let tty = opts
        .tty
        .unwrap_or_else(|| env::var("TERM").as_deref() == Ok("linux"));
    let quote_files = if opts.quote || opts.quoters {
        names(&["quotes", "ponyquotes"], &dirs)
    } else {
        Vec::new()
    };
    if opts.balloons {
        return Ok(names(&["balloons"], &dirs).join("\n") + "\n");
    }
    if opts.list.is_some() || opts.quoters {
        let all = names(&group_names(opts.list.unwrap_or(0), tty), &dirs);
        let selected: Vec<_> = all
            .iter()
            .filter(|n| {
                !opts.quoters || has_quotes(n, opts.list.unwrap_or(0), tty, &dirs, &quote_files)
            })
            .map(|n| n.trim_end_matches(".pony"))
            .collect();
        return Ok(selected.join("\n") + "\n");
    }
    let p = if opts.files.is_empty() {
        let candidates: Vec<_> = names(&group_names(0, tty), &dirs)
            .into_iter()
            .filter(|n| !opts.quote || has_quotes(n, 0, tty, &dirs, &quote_files))
            .collect();
        pony(choose(&candidates)?, 0, tty, &dirs)?
    } else {
        // Validate every explicitly supplied candidate, even when only one is drawn.
        let choices: Vec<_> = opts
            .files
            .iter()
            .map(|(name, group)| pony(name, *group, tty, &dirs))
            .collect::<Result<_, _>>()?;
        choose(&choices)?.clone()
    };
    if opts.info == 1 {
        let mut output = metadata(&p.data).to_owned() + "\n";
        if opts.no_color {
            output = strip_ansi(&output);
        }
        return Ok(output);
    }
    let message = if opts.info == 2 {
        metadata(&p.data).to_owned()
    } else if opts.quote {
        if !opts.message.is_empty() {
            return Err("a quote and an explicit message cannot be combined".into());
        }
        let mut candidates = quotes_for(&quote_files, &p.canonical);
        if candidates.is_empty() {
            candidates = quotes_for(&quote_files, &p.name);
        }
        let file = choose(&candidates).map_err(|_| format!("no quotes for {:?}", p.name))?;
        load(file, &["quotes", "ponyquotes"], &dirs)?
            .1
            .trim()
            .to_owned()
    } else if !opts.message.is_empty() {
        opts.message.join(" ")
    } else if opts.only {
        String::new()
    } else if io::stdin().is_terminal() {
        return Ok(HELP.into());
    } else {
        let mut s = String::new();
        io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| e.to_string())?;
        s.trim_end_matches(['\r', '\n']).to_owned()
    };
    let terminal_width = terminal_size::terminal_size()
        .map(|(w, _)| usize::from(w.0))
        .or_else(|| env::var("COLUMNS").ok()?.parse().ok())
        .unwrap_or(80);
    let wrap = match opts.wrap.as_deref().unwrap_or("65") {
        "n" | "N" | "m" | "M" | "s" | "S" => None,
        "i" | "I" | "o" | "O" | "u" | "U" => Some(terminal_width),
        s => {
            let n = s
                .parse::<usize>()
                .map_err(|_| "wrap width must be a positive integer, n, or i")?;
            if n == 0 || n > 16_384 {
                return Err("wrap width must be 1..16384".into());
            }
            Some(n)
        }
    };
    let balloon = if let Some(name) = opts.balloon {
        Some(if Path::new(&name).is_file() {
            load_path(Path::new(&name))?.1
        } else {
            let file = if name.ends_with(".say") || name.ends_with(".think") {
                name
            } else {
                format!("{}.{}", name, if opts.think { "think" } else { "say" })
            };
            load(&file, &["balloons"], &dirs)?.1
        })
    } else {
        None
    };
    let full_width = env::var("PONYSAY_FULL_WIDTH")
        .map(|s| ["yes", "y", "1"].contains(&s.as_str()))
        .unwrap_or(false);
    let mut output = render(
        &p.data,
        &message,
        &RenderOptions {
            wrap,
            think: opts.think,
            pony_only: opts.only,
            width: if full_width || !io::stdout().is_terminal() {
                None
            } else {
                Some(terminal_width)
            },
            balloon,
            compact: opts.compact,
        },
    )?;
    if opts.no_color {
        output = strip_ansi(&output);
    }
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn main() {
    let args = env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "arguments must be valid UTF-8".to_owned());
    let output = match args.and_then(parse) {
        Ok(opts) => run(opts),
        Err(s) if s.starts_with("ponysay-rust ") => Ok(s + "\n"),
        Err(s) => Err(s),
    };
    match output {
        Ok(s) => {
            if let Err(e) = io::stdout().lock().write_all(s.as_bytes()) {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("ponysay: {e}");
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("ponysay: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_message() {
        let o = parse(vec!["--".into(), r#"$HOME C:\ponies 'hello' \-"#.into()]).unwrap();
        assert_eq!(o.message[0], r#"$HOME C:\ponies 'hello' \-"#);
    }
    #[test]
    fn options_and_aliases() {
        let o = parse(vec![
            "-ftwilight".into(),
            "-W20".into(),
            "--think".into(),
            "hello".into(),
        ])
        .unwrap();
        assert_eq!(o.files[0].0, "twilight");
        assert_eq!(o.wrap.as_deref(), Some("20"));
        assert!(o.think);
    }
    #[test]
    fn reject_missing_argument() {
        assert!(parse(vec!["-f".into()]).is_err());
    }
    #[test]
    fn unicode_argument() {
        assert_eq!(parse(vec!["世界 🌈".into()]).unwrap().message[0], "世界 🌈");
    }
    #[test]
    fn clustered_short_flags_and_unicode_values() {
        let o = parse(vec!["-of".into(), "pinkie".into()]).unwrap();
        assert!(o.only);
        assert_eq!(o.files[0], ("pinkie".to_owned(), 0));
        let o = parse(vec!["-ocf世界".into(), "--".into(), "-literal".into()]).unwrap();
        assert!(o.only && o.compact);
        assert_eq!(o.files[0].0, "世界");
        assert_eq!(o.message, ["-literal"]);
        assert!(parse(vec!["-oZ".into()]).is_err());
        assert!(parse(vec!["-o界".into()]).is_err());
    }
    #[test]
    fn plural_inline_values_are_not_ignored() {
        let o = parse(vec![
            "--ponies=twilight".into(),
            "pinkie".into(),
            "--".into(),
            "hello".into(),
        ])
        .unwrap();
        assert_eq!(o.files, [("twilight".into(), 0), ("pinkie".into(), 0)]);
        assert_eq!(o.message, ["hello"]);
        let o = parse(vec!["++ponies=custom".into()]).unwrap();
        assert_eq!(o.files, [("custom".into(), 1)]);
        assert!(parse(vec!["--no-color=no".into()]).is_err());
    }
    #[test]
    fn strip_colors_and_links() {
        assert_eq!(
            strip_ansi("\x1b[31mhello\x1b[0m \x1b]8;;https://example.org\x1b\\link\x1b]8;;\x1b\\"),
            "hello link"
        );
        assert_eq!(
            strip_ansi("\x1b]PA5FAF00\x1b[32mPONY\x1b]R tail"),
            "PONY tail"
        );
        assert_eq!(strip_ansi("\x1bPpayload\x1b\\PONY"), "PONY");
    }
    #[test]
    fn embedded_assets_and_aliases() {
        assert!(ASSETS.len() > 1500);
        let p = pony("twilight", 0, false, &[]).unwrap();
        assert!(p.data.contains("NAME: Twilight"));
        assert!(!names(&["balloons"], &[]).is_empty());
    }
    #[test]
    fn shared_quotes() {
        let q = vec!["twilight+pinkie.1".into(), "twilight.2".into()];
        assert_eq!(quotes_for(&q, "twilight").len(), 2);
        assert_eq!(quotes_for(&q, "twili").len(), 0);
    }
    #[test]
    fn custom_assets_override() {
        let dir = env::temp_dir().join(format!("ponysay-{}", std::process::id()));
        fs::create_dir_all(dir.join("ponies")).unwrap();
        fs::write(dir.join("ponies/twilight.pony"), "custom").unwrap();
        assert_eq!(
            pony("twilight", 0, false, std::slice::from_ref(&dir))
                .unwrap()
                .data,
            "custom"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
