use crate::model::{parse_clock, RepeatMode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Amount {
    Abs(f64),
    Delta(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Search(String),
    Play(Option<usize>),
    Pause,
    Resume,
    Toggle,
    Stop,
    Next,
    Prev,
    Quit,
    Add(Option<usize>),
    AddAll,
    PlayAll,
    Remove(Option<usize>),
    Clear,
    ShowResults,
    ShowQueue,
    ShowPlaylists,
    Volume(Amount),
    Seek(Amount),
    Shuffle,
    Repeat(Option<RepeatMode>),
    Save(String),
    Load(String),
    Append(String),
    DeletePlaylist(String),
    AddTo(String),
    Import(String),
    Artist,
    Help,
}

pub const COMMAND_NAMES: &[&str] = &[
    "search", "play", "pause", "resume", "toggle", "stop", "next", "prev", "quit", "add",
    "addall", "playall", "remove", "clear", "results", "queue", "playlists", "volume", "seek",
    "shuffle", "repeat", "save", "load", "append", "delpl", "addto", "import", "artist", "help",
];

fn unquote(s: &str) -> &str {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn index_arg(arg: &str, cmd: &str) -> Result<Option<usize>, String> {
    if arg.is_empty() {
        return Ok(None);
    }
    match arg.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(Some(n)),
        _ => Err(format!(":{cmd} expects a row number (1, 2, 3 …), got '{arg}'")),
    }
}

fn name_arg(arg: &str, cmd: &str) -> Result<String, String> {
    let a = unquote(arg);
    if a.is_empty() {
        Err(format!("usage: :{cmd} <name>"))
    } else {
        Ok(a.to_string())
    }
}

fn parse_amount(arg: &str, cmd: &str) -> Result<Amount, String> {
    let a = arg.trim().trim_end_matches('%');
    if a.is_empty() {
        return Err(format!("usage: :{cmd} <value>  (e.g. 50, +10, -10)"));
    }
    let err = || format!(":{cmd}: cannot understand '{arg}'");
    if let Some(rest) = a.strip_prefix('+') {
        Ok(Amount::Delta(parse_clock(rest).ok_or_else(err)?))
    } else if let Some(rest) = a.strip_prefix('-') {
        Ok(Amount::Delta(-parse_clock(rest).ok_or_else(err)?))
    } else {
        Ok(Amount::Abs(parse_clock(a).ok_or_else(err)?))
    }
}

pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let line = line.trim().trim_start_matches(':').trim();
    if line.is_empty() {
        return Ok(None);
    }
    let end = line
        .find(|c: char| !(c.is_ascii_alphabetic() || c == '!'))
        .unwrap_or(line.len());
    let (name, rest) = line.split_at(end);
    let rest = rest.trim();
    let arg = unquote(rest);

    use Command::*;
    let cmd = match name.to_ascii_lowercase().as_str() {
        "search" | "s" | "find" => {
            if arg.is_empty() {
                return Err("usage: :search \"your query\"".into());
            }
            Search(arg.to_string())
        }
        "play" => Play(index_arg(arg, "play")?),
        "pause" => Pause,
        "resume" | "unpause" => Resume,
        "toggle" => Toggle,
        "stop" => Stop,
        "next" | "n" => Next,
        "prev" | "previous" => Prev,
        "quit" | "q" | "exit" | "q!" | "qa" | "wq" => Quit,
        "add" | "a" => Add(index_arg(arg, "add")?),
        "addall" => AddAll,
        "playall" => PlayAll,
        "remove" | "rm" | "del" => Remove(index_arg(arg, "remove")?),
        "clear" => Clear,
        "results" | "res" => ShowResults,
        "queue" => ShowQueue,
        "playlists" | "pls" => ShowPlaylists,
        "volume" | "vol" | "v" => Volume(parse_amount(arg, "volume")?),
        "seek" => Seek(parse_amount(arg, "seek")?),
        "shuffle" => Shuffle,
        "repeat" => match arg.to_ascii_lowercase().as_str() {
            "" => Repeat(None),
            "off" | "none" | "no" => Repeat(Some(RepeatMode::Off)),
            "all" | "queue" => Repeat(Some(RepeatMode::All)),
            "one" | "track" | "1" => Repeat(Some(RepeatMode::One)),
            other => return Err(format!(":repeat expects off|all|one, got '{other}'")),
        },
        "save" | "w" => Save(name_arg(arg, "save")?),
        "load" | "e" => Load(name_arg(arg, "load")?),
        "append" => Append(name_arg(arg, "append")?),
        "delpl" | "deletepl" | "delete" => DeletePlaylist(name_arg(arg, "delpl")?),
        "addto" => AddTo(name_arg(arg, "addto")?),
        "import" => {
            if arg.is_empty() {
                return Err("usage: :import <youtube / youtube music playlist URL>".into());
            }
            Import(arg.to_string())
        }
        "artist" | "ar" => Artist,
        "help" | "h" => Help,
        other => return Err(format!("unknown command: :{other}  (try :help)")),
    };
    Ok(Some(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_variants() {
        assert_eq!(parse(r#"search "daft punk""#), Ok(Some(Command::Search("daft punk".into()))));
        assert_eq!(parse(":search daft punk"), Ok(Some(Command::Search("daft punk".into()))));
        assert_eq!(parse(r#"s"lofi beats""#), Ok(Some(Command::Search("lofi beats".into()))));
        assert!(parse("search").is_err());
    }

    #[test]
    fn simple_commands() {
        assert_eq!(parse("play"), Ok(Some(Command::Play(None))));
        assert_eq!(parse("play 3"), Ok(Some(Command::Play(Some(3)))));
        assert!(parse("play 0").is_err());
        assert_eq!(parse("q!"), Ok(Some(Command::Quit)));
        assert_eq!(parse("pause"), Ok(Some(Command::Pause)));
        assert_eq!(parse("   "), Ok(None));
        assert!(parse("bogus").is_err());
    }

    #[test]
    fn amounts() {
        assert_eq!(parse("vol 50"), Ok(Some(Command::Volume(Amount::Abs(50.0)))));
        assert_eq!(parse("vol -10"), Ok(Some(Command::Volume(Amount::Delta(-10.0)))));
        assert_eq!(parse("seek 1:30"), Ok(Some(Command::Seek(Amount::Abs(90.0)))));
        assert_eq!(parse("seek +15"), Ok(Some(Command::Seek(Amount::Delta(15.0)))));
        assert!(parse("vol loud").is_err());
    }

    #[test]
    fn artist_command() {
        assert_eq!(parse("artist"), Ok(Some(Command::Artist)));
        assert_eq!(parse(":ar"), Ok(Some(Command::Artist)));
    }

    #[test]
    fn playlists() {
        assert_eq!(parse(r#"save "my mix""#), Ok(Some(Command::Save("my mix".into()))));
        assert!(parse("save").is_err());
    }
}
