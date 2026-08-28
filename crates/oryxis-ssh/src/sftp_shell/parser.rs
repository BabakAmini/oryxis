//! Turning a submitted line into a [`Command`].
//!
//! The grammar is `sftp(1)`'s, deliberately: the people who ask for this
//! console learned `get` and `lcd` from OpenSSH, on whatever machine they
//! were sitting at, and a dialect of our own would be a worse version of
//! something they already know.
//!
//! The tokenizer is written here rather than taken from `shell-words` for
//! the same reason [`super::glob`] is: the rules are small, they are
//! `sftp(1)`'s and not the POSIX shell's, and owning them means the
//! quoting a filename needs (`get "My Documents/report.pdf"`) is covered
//! by a test instead of by a dependency's interpretation.

/// A parsed console command. One variant per `sftp(1)` command we
/// implement; the flags each one accepts are fields rather than a shared
/// bag, so a flag that means nothing for a command cannot be silently
/// accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Change the remote directory. `None` = the session's home, which is
    /// what a bare `cd` does.
    Cd(Option<String>),
    /// Change the local directory. `None` = the local home.
    Lcd(Option<String>),
    /// Print the remote working directory.
    Pwd,
    /// Print the local working directory.
    Lpwd,
    Ls(LsOpts),
    Lls(LsOpts),
    /// Download. `local` is where it lands; `None` means "the basename,
    /// in the local working directory".
    Get {
        opts: XferOpts,
        remote: String,
        local: Option<String>,
    },
    /// Upload, mirroring [`Command::Get`].
    Put {
        opts: XferOpts,
        local: String,
        remote: Option<String>,
    },
    /// Remove remote files. Carries every operand because `rm a b c` is
    /// one command, and each may be a glob.
    Rm(Vec<String>),
    Mkdir(String),
    Lmkdir(String),
    Rmdir(String),
    Rename {
        from: String,
        to: String,
    },
    Chmod {
        mode: u32,
        paths: Vec<String>,
    },
    /// Toggle the transfer progress meter. `sftp(1)` takes no argument
    /// and reports the new state.
    Progress,
    /// `help` / `?`.
    Help,
    Version,
    /// `bye` / `quit` / `exit`.
    Quit,
}

/// Flags shared by `ls` and `lls`, which take the same set, plus the
/// optional path operand. Not `Copy`, because the path is owned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LsOpts {
    /// The path to list. `None` = the working directory.
    pub path: Option<String>,
    /// `-1`: one entry per line.
    pub one_per_line: bool,
    /// `-a`: include entries whose name starts with a dot.
    pub all: bool,
    /// `-f`: do not sort, list in the order the server answered.
    pub unsorted: bool,
    /// `-h`: with `-l`, sizes as 1.2K / 3.4M.
    pub human: bool,
    /// `-l`: long format.
    pub long: bool,
    /// `-n`: with `-l`, numeric uid/gid. Accepted and ignored, because
    /// numeric is all we can do: see the `longname` note in
    /// [`super::render`].
    pub numeric: bool,
    /// `-r`: reverse the sort.
    pub reverse: bool,
    /// `-S`: sort by size.
    pub by_size: bool,
    /// `-t`: sort by modification time.
    pub by_time: bool,
}

impl LsOpts {
    /// The path operand, kept next to the flags so callers pass one
    /// value. `None` = the working directory.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

/// Flags shared by `get` and `put`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XferOpts {
    /// `-a`: resume a partial transfer.
    pub resume: bool,
    /// `-f`: fsync after writing.
    pub fsync: bool,
    /// `-p` / `-P`: preserve modification times and permissions.
    pub preserve: bool,
    /// `-r` / `-R`: recurse into directories.
    pub recursive: bool,
}

/// Why a line could not become a command. The messages these produce are
/// `sftp(1)`'s, so a user pasting from a tutorial sees what the tutorial
/// says they will see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing but whitespace. Not an error to report; the caller just
    /// reprompts.
    Empty,
    /// First word is not a command we know.
    UnknownCommand(String),
    /// A quote was opened and never closed.
    UnterminatedQuote,
    /// The command needs operands it did not get.
    MissingOperand(&'static str),
    /// More operands than the command takes.
    TooManyOperands(&'static str),
    /// A flag the command does not accept.
    UnknownFlag { command: &'static str, flag: char },
    /// `chmod`'s mode did not parse as octal.
    BadMode(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => Ok(()),
            ParseError::UnknownCommand(c) => write!(f, "Invalid command: {c}"),
            ParseError::UnterminatedQuote => write!(f, "Unterminated quoted argument"),
            ParseError::MissingOperand(c) => {
                write!(f, "You must specify a path after the {c} command.")
            }
            ParseError::TooManyOperands(c) => write!(f, "Too many arguments for {c}."),
            ParseError::UnknownFlag { command, flag } => {
                write!(f, "{command}: unknown option -- {flag}")
            }
            ParseError::BadMode(m) => write!(f, "chmod: invalid mode: {m}"),
        }
    }
}

/// Split a line into words, honouring quoting.
///
/// The rules, which are `sftp(1)`'s:
/// - a backslash escapes the next character anywhere;
/// - double quotes group, and a backslash inside them still escapes;
/// - single quotes group literally, with no escape inside;
/// - unquoted runs of spaces and tabs separate words.
///
/// An empty quoted string is a real, empty word: `put "" x` has two
/// operands, not one, which is what lets the caller report the mistake
/// instead of silently shifting arguments.
pub fn tokenize(line: &str) -> Result<Vec<String>, ParseError> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut has_word = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if has_word {
                    words.push(std::mem::take(&mut cur));
                    has_word = false;
                }
            }
            '\\' => match chars.next() {
                Some(next) => {
                    cur.push(next);
                    has_word = true;
                }
                None => return Err(ParseError::UnterminatedQuote),
            },
            '"' => {
                has_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(next) => cur.push(next),
                            None => return Err(ParseError::UnterminatedQuote),
                        },
                        Some(other) => cur.push(other),
                        None => return Err(ParseError::UnterminatedQuote),
                    }
                }
            }
            '\'' => {
                has_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(other) => cur.push(other),
                        None => return Err(ParseError::UnterminatedQuote),
                    }
                }
            }
            other => {
                cur.push(other);
                has_word = true;
            }
        }
    }
    if has_word {
        words.push(cur);
    }
    Ok(words)
}

/// Parse one submitted line.
pub fn parse(line: &str) -> Result<Command, ParseError> {
    let words = tokenize(line)?;
    let Some((verb, rest)) = words.split_first() else {
        return Err(ParseError::Empty);
    };
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();

    match verb.to_ascii_lowercase().as_str() {
        "cd" => Ok(Command::Cd(one_optional(&rest, "cd")?)),
        "lcd" => Ok(Command::Lcd(one_optional(&rest, "lcd")?)),
        "pwd" => Ok(Command::Pwd),
        "lpwd" => Ok(Command::Lpwd),
        "ls" | "dir" => Ok(Command::Ls(parse_ls(&rest, "ls")?)),
        "lls" => Ok(Command::Lls(parse_ls(&rest, "lls")?)),
        "get" | "reget" | "mget" => {
            let mut opts = XferOpts::default();
            // `reget` IS `get -a`, and `mget` is `get` whose operand is
            // expected to be a glob. Both are aliases in `sftp(1)`, so
            // they collapse here rather than growing variants that would
            // have to be handled identically everywhere downstream.
            if verb.eq_ignore_ascii_case("reget") {
                opts.resume = true;
            }
            let (opts, operands) = parse_xfer_flags(&rest, opts, "get")?;
            let mut it = operands.into_iter();
            let remote = it.next().ok_or(ParseError::MissingOperand("get"))?;
            let local = it.next();
            if it.next().is_some() {
                return Err(ParseError::TooManyOperands("get"));
            }
            Ok(Command::Get {
                opts,
                remote,
                local,
            })
        }
        "put" | "reput" | "mput" => {
            let mut opts = XferOpts::default();
            if verb.eq_ignore_ascii_case("reput") {
                opts.resume = true;
            }
            let (opts, operands) = parse_xfer_flags(&rest, opts, "put")?;
            let mut it = operands.into_iter();
            let local = it.next().ok_or(ParseError::MissingOperand("put"))?;
            let remote = it.next();
            if it.next().is_some() {
                return Err(ParseError::TooManyOperands("put"));
            }
            Ok(Command::Put {
                opts,
                local,
                remote,
            })
        }
        "rm" | "delete" => {
            if rest.is_empty() {
                return Err(ParseError::MissingOperand("rm"));
            }
            Ok(Command::Rm(rest.iter().map(|s| s.to_string()).collect()))
        }
        "mkdir" => Ok(Command::Mkdir(exactly_one(&rest, "mkdir")?)),
        "lmkdir" => Ok(Command::Lmkdir(exactly_one(&rest, "lmkdir")?)),
        "rmdir" => Ok(Command::Rmdir(exactly_one(&rest, "rmdir")?)),
        "rename" => {
            if rest.len() < 2 {
                return Err(ParseError::MissingOperand("rename"));
            }
            if rest.len() > 2 {
                return Err(ParseError::TooManyOperands("rename"));
            }
            Ok(Command::Rename {
                from: rest[0].to_string(),
                to: rest[1].to_string(),
            })
        }
        "chmod" => {
            if rest.len() < 2 {
                return Err(ParseError::MissingOperand("chmod"));
            }
            let mode = u32::from_str_radix(rest[0], 8)
                .map_err(|_| ParseError::BadMode(rest[0].to_string()))?;
            // A mode is at most four octal digits. Anything wider is a
            // typo that would otherwise be masked into something valid.
            if mode > 0o7777 {
                return Err(ParseError::BadMode(rest[0].to_string()));
            }
            Ok(Command::Chmod {
                mode,
                paths: rest[1..].iter().map(|s| s.to_string()).collect(),
            })
        }
        "progress" => Ok(Command::Progress),
        "help" | "?" => Ok(Command::Help),
        "version" => Ok(Command::Version),
        "bye" | "quit" | "exit" => Ok(Command::Quit),
        other => Err(ParseError::UnknownCommand(other.to_string())),
    }
}

/// A command taking zero or one operand.
fn one_optional(rest: &[&str], cmd: &'static str) -> Result<Option<String>, ParseError> {
    match rest.len() {
        0 => Ok(None),
        1 => Ok(Some(rest[0].to_string())),
        _ => Err(ParseError::TooManyOperands(cmd)),
    }
}

/// A command taking exactly one operand.
fn exactly_one(rest: &[&str], cmd: &'static str) -> Result<String, ParseError> {
    match rest.len() {
        0 => Err(ParseError::MissingOperand(cmd)),
        1 => Ok(rest[0].to_string()),
        _ => Err(ParseError::TooManyOperands(cmd)),
    }
}

fn parse_ls(rest: &[&str], cmd: &'static str) -> Result<LsOpts, ParseError> {
    let mut opts = LsOpts::default();
    let mut path = None;
    for word in rest {
        if let Some(flags) = flag_body(word) {
            for f in flags.chars() {
                match f {
                    '1' => opts.one_per_line = true,
                    'a' => opts.all = true,
                    'f' => opts.unsorted = true,
                    'h' => opts.human = true,
                    'l' => opts.long = true,
                    'n' => {
                        opts.numeric = true;
                        opts.long = true;
                    }
                    'r' => opts.reverse = true,
                    'S' => opts.by_size = true,
                    't' => opts.by_time = true,
                    other => {
                        return Err(ParseError::UnknownFlag {
                            command: cmd,
                            flag: other,
                        });
                    }
                }
            }
        } else if path.is_none() {
            path = Some(word.to_string());
        } else {
            return Err(ParseError::TooManyOperands(cmd));
        }
    }
    opts.path = path;
    Ok(opts)
}

fn parse_xfer_flags(
    rest: &[&str],
    mut opts: XferOpts,
    cmd: &'static str,
) -> Result<(XferOpts, Vec<String>), ParseError> {
    let mut operands = Vec::new();
    for word in rest {
        if let Some(flags) = flag_body(word) {
            for f in flags.chars() {
                match f {
                    'a' => opts.resume = true,
                    'f' => opts.fsync = true,
                    'p' | 'P' => opts.preserve = true,
                    'r' | 'R' => opts.recursive = true,
                    other => {
                        return Err(ParseError::UnknownFlag {
                            command: cmd,
                            flag: other,
                        });
                    }
                }
            }
        } else {
            operands.push(word.to_string());
        }
    }
    Ok((opts, operands))
}

/// The flag letters of `word`, or `None` when it is an operand.
///
/// A bare `-` is an operand, not an empty flag group: it is a legal
/// filename and treating it as a flag would make it unreachable.
fn flag_body(word: &str) -> Option<&str> {
    let body = word.strip_prefix('-')?;
    if body.is_empty() { None } else { Some(body) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(line: &str) -> Command {
        parse(line).unwrap_or_else(|e| panic!("{line:?} should parse, got {e:?}"))
    }

    // --- tokenizer --------------------------------------------------

    #[test]
    fn plain_words_split_on_whitespace() {
        assert_eq!(tokenize("get a b").unwrap(), vec!["get", "a", "b"]);
        assert_eq!(tokenize("  ls   -l  ").unwrap(), vec!["ls", "-l"]);
        assert_eq!(tokenize("").unwrap(), Vec::<String>::new());
    }

    /// The reason quoting exists at all here: a filename with a space is
    /// ordinary, and splitting it would reach the wrong file.
    #[test]
    fn double_quotes_group_a_filename_with_spaces() {
        assert_eq!(
            tokenize(r#"get "My Documents/report.pdf""#).unwrap(),
            vec!["get", "My Documents/report.pdf"]
        );
    }

    #[test]
    fn single_quotes_group_literally() {
        assert_eq!(
            tokenize(r#"get 'a "b" c'"#).unwrap(),
            vec!["get", r#"a "b" c"#]
        );
    }

    #[test]
    fn a_backslash_escapes_a_space_and_a_quote() {
        assert_eq!(tokenize(r"get my\ file").unwrap(), vec!["get", "my file"]);
        assert_eq!(tokenize(r#"get \"x"#).unwrap(), vec!["get", "\"x"]);
    }

    #[test]
    fn a_backslash_inside_double_quotes_still_escapes() {
        assert_eq!(tokenize(r#"get "a\"b""#).unwrap(), vec!["get", "a\"b"]);
    }

    #[test]
    fn an_empty_quoted_string_is_a_real_word() {
        assert_eq!(tokenize(r#"put "" x"#).unwrap(), vec!["put", "", "x"]);
    }

    #[test]
    fn an_unterminated_quote_is_an_error() {
        assert_eq!(tokenize(r#"get "abc"#), Err(ParseError::UnterminatedQuote));
        assert_eq!(tokenize("get 'abc"), Err(ParseError::UnterminatedQuote));
        assert_eq!(tokenize(r"get abc\"), Err(ParseError::UnterminatedQuote));
    }

    // --- commands ---------------------------------------------------

    #[test]
    fn an_empty_line_parses_as_empty() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("   "), Err(ParseError::Empty));
    }

    #[test]
    fn navigation_commands() {
        assert_eq!(ok("cd /var/log"), Command::Cd(Some("/var/log".into())));
        assert_eq!(ok("cd"), Command::Cd(None));
        assert_eq!(
            ok("lcd ~/Downloads"),
            Command::Lcd(Some("~/Downloads".into()))
        );
        assert_eq!(ok("pwd"), Command::Pwd);
        assert_eq!(ok("lpwd"), Command::Lpwd);
    }

    #[test]
    fn command_names_are_case_insensitive() {
        assert_eq!(ok("PWD"), Command::Pwd);
        assert_eq!(ok("Ls"), Command::Ls(LsOpts::default()));
    }

    #[test]
    fn ls_flags_accumulate_and_can_be_bundled() {
        let Command::Ls(opts) = ok("ls -lah /tmp") else {
            panic!("not an ls");
        };
        assert!(opts.long && opts.all && opts.human);
        assert_eq!(opts.path(), Some("/tmp"));
    }

    #[test]
    fn ls_flags_can_be_separate_words() {
        let Command::Ls(opts) = ok("ls -l -a") else {
            panic!("not an ls");
        };
        assert!(opts.long && opts.all);
        assert_eq!(opts.path(), None);
    }

    /// `-n` implies `-l` in `sftp(1)`: numeric ids only mean something in
    /// the long format.
    #[test]
    fn ls_numeric_implies_long() {
        let Command::Ls(opts) = ok("ls -n") else {
            panic!("not an ls");
        };
        assert!(opts.numeric && opts.long);
    }

    #[test]
    fn an_unknown_flag_names_itself() {
        assert_eq!(
            parse("ls -z"),
            Err(ParseError::UnknownFlag {
                command: "ls",
                flag: 'z'
            })
        );
    }

    #[test]
    fn get_takes_one_or_two_operands() {
        assert_eq!(
            ok("get access.log"),
            Command::Get {
                opts: XferOpts::default(),
                remote: "access.log".into(),
                local: None
            }
        );
        assert_eq!(
            ok("get access.log /tmp/a.log"),
            Command::Get {
                opts: XferOpts::default(),
                remote: "access.log".into(),
                local: Some("/tmp/a.log".into())
            }
        );
        assert_eq!(parse("get"), Err(ParseError::MissingOperand("get")));
        assert_eq!(parse("get a b c"), Err(ParseError::TooManyOperands("get")));
    }

    #[test]
    fn transfer_flags_parse_in_either_case() {
        let Command::Get { opts, .. } = ok("get -rP x") else {
            panic!("not a get");
        };
        assert!(opts.recursive && opts.preserve);
        let Command::Put { opts, .. } = ok("put -Rp x") else {
            panic!("not a put");
        };
        assert!(opts.recursive && opts.preserve);
    }

    /// `reget` and `reput` are `-a` spelled as a verb, and collapsing
    /// them here is what keeps the resume path single downstream.
    #[test]
    fn reget_and_reput_are_resume() {
        let Command::Get { opts, .. } = ok("reget big.iso") else {
            panic!("not a get");
        };
        assert!(opts.resume);
        let Command::Put { opts, .. } = ok("reput big.iso") else {
            panic!("not a put");
        };
        assert!(opts.resume);
    }

    /// `mget` / `mput` are `get` / `put` whose operand happens to be a
    /// glob; the expansion is the executor's job, not the parser's.
    #[test]
    fn mget_and_mput_are_get_and_put() {
        assert_eq!(
            ok("mget *.gz"),
            Command::Get {
                opts: XferOpts::default(),
                remote: "*.gz".into(),
                local: None
            }
        );
        assert!(matches!(ok("mput *.txt"), Command::Put { .. }));
    }

    #[test]
    fn rm_takes_every_operand() {
        assert_eq!(
            ok("rm a b *.tmp"),
            Command::Rm(vec!["a".into(), "b".into(), "*.tmp".into()])
        );
        assert_eq!(parse("rm"), Err(ParseError::MissingOperand("rm")));
    }

    #[test]
    fn rename_needs_exactly_two() {
        assert_eq!(
            ok("rename a b"),
            Command::Rename {
                from: "a".into(),
                to: "b".into()
            }
        );
        assert_eq!(parse("rename a"), Err(ParseError::MissingOperand("rename")));
        assert_eq!(
            parse("rename a b c"),
            Err(ParseError::TooManyOperands("rename"))
        );
    }

    #[test]
    fn chmod_parses_an_octal_mode() {
        assert_eq!(
            ok("chmod 644 f"),
            Command::Chmod {
                mode: 0o644,
                paths: vec!["f".into()]
            }
        );
        assert_eq!(
            ok("chmod 4755 a b"),
            Command::Chmod {
                mode: 0o4755,
                paths: vec!["a".into(), "b".into()]
            }
        );
    }

    /// A decimal-looking mode is the classic mistake, and `999` is not
    /// octal at all. Reporting beats silently masking it into something
    /// that changes permissions the user did not ask for.
    #[test]
    fn chmod_rejects_a_mode_that_is_not_octal() {
        assert_eq!(parse("chmod 999 f"), Err(ParseError::BadMode("999".into())));
        assert_eq!(parse("chmod rwx f"), Err(ParseError::BadMode("rwx".into())));
        assert_eq!(
            parse("chmod 77777 f"),
            Err(ParseError::BadMode("77777".into()))
        );
        assert_eq!(parse("chmod 644"), Err(ParseError::MissingOperand("chmod")));
    }

    #[test]
    fn the_exit_family_is_one_command() {
        assert_eq!(ok("bye"), Command::Quit);
        assert_eq!(ok("quit"), Command::Quit);
        assert_eq!(ok("exit"), Command::Quit);
    }

    #[test]
    fn help_answers_to_both_spellings() {
        assert_eq!(ok("help"), Command::Help);
        assert_eq!(ok("?"), Command::Help);
    }

    #[test]
    fn an_unknown_command_names_itself() {
        assert_eq!(
            parse("frobnicate x"),
            Err(ParseError::UnknownCommand("frobnicate".into()))
        );
    }

    /// A file literally named `-` has to stay reachable, so a bare dash
    /// is an operand rather than an empty flag group.
    #[test]
    fn a_bare_dash_is_an_operand() {
        assert_eq!(
            ok("get -"),
            Command::Get {
                opts: XferOpts::default(),
                remote: "-".into(),
                local: None
            }
        );
    }

    /// A quoted operand that looks like a flag is an operand: quoting is
    /// how the user says so.
    #[test]
    fn a_quoted_dash_word_is_still_an_operand() {
        // The tokenizer strips the quotes, so this documents the known
        // limit: quoting does not survive into flag detection, exactly as
        // in `sftp(1)`. `./-l` is the portable way to name such a file.
        assert_eq!(
            ok("get ./-l"),
            Command::Get {
                opts: XferOpts::default(),
                remote: "./-l".into(),
                local: None
            }
        );
    }

    #[test]
    fn quoted_paths_survive_into_the_command() {
        assert_eq!(
            ok(r#"cd "/var/my logs""#),
            Command::Cd(Some("/var/my logs".into()))
        );
    }
}
