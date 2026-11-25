use super::*;

/// A small definition that holds to every rule; each test below breaks one.
const BASE: &str = r#"
name = "toy"
format = 1

[detect]
programs = ["toy"]
command_line = ["toy-agent"]

[events]
ready = ["idle"]
busy = ["busy"]

[setup]
dir = "~/.toy"

[[setup.files]]
path = "hooks.toml"
how = "append"
mark = "keepane pane-ready"
head_mark = "[hooks]"
head = "[hooks]\non = true\n"
text = '''
idle = "{ready}"
busy = "{busy}"
'''
"#;

fn base_with(from: &str, to: &str) -> String {
    assert!(BASE.contains(from), "the base holds {from:?}");
    BASE.replacen(from, to, 1)
}

#[test]
fn the_shipped_definitions_load() {
    let r = Registry::load(&[]);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let cw = r.get("codewhale").expect("codewhale is shipped");
    assert_eq!((cw.title(), cw.from.as_str()), ("CodeWhale", "agents/codewhale.toml"));
    assert_eq!(cw.events.ready, ["session_start", "session_idle"]);
    assert_eq!(cw.resume.as_deref(), Some("codewhale resume {id}"));
    let hooks = &cw.setup.files[0];
    let text = cw.text_of(hooks);
    assert!(text.contains("command = \"keepane pane-ready -q --agent codewhale\""), "{text}");
    assert!(text.contains("command = \"keepane pane-busy -q --agent codewhale\""), "{text}");
    assert!(text.contains("command = \"keepane pane-wait -q --agent codewhale\""), "{text}");
    assert!(!text.contains('{'), "every placeholder filled: {text}");
    assert_eq!(cw.head_of(hooks).as_deref(), Some("[hooks]\nenabled = true\n"));
    assert_eq!(cw.setup.files[1].at.as_ref().unwrap().all(), ["/servers/keepane", "/mcpServers/keepane"]);
}

/// A file with CRLF line breaks (a Windows editor; the shipped file
/// checked out on Windows) reads as the same definition: no CR in what
/// `setup` writes.
#[test]
fn crlf_reads_as_lf() {
    let crlf = BASE.replace("\r\n", "\n").replace('\n', "\r\n");
    let (a, b) = (parse(BASE, "lf").unwrap(), parse(&crlf, "crlf").unwrap());
    let f = (&a.setup.files[0], &b.setup.files[0]);
    assert_eq!(b.text_of(f.1), a.text_of(f.0));
    assert_eq!(b.head_of(f.1), a.head_of(f.0));
    assert!(!b.text_of(f.1).contains('\r') && !b.head_of(f.1).unwrap().contains('\r'));
    let shipped = SHIPPED[0].1.replace("\r\n", "\n").replace('\n', "\r\n");
    let cw = parse(&shipped, "crlf codewhale").unwrap();
    assert!(!cw.text_of(&cw.setup.files[0]).contains('\r'));
}

#[test]
fn a_program_is_known_by_its_stem() {
    assert_eq!(program_stem(r"C:\Users\me\AppData\Roaming\npm\codew.CMD"), "codew");
    assert_eq!(program_stem("/usr/local/bin/codewhale"), "codewhale");
    assert_eq!(program_stem("codewhale.exe"), "codewhale");
    assert_eq!(program_stem("tool.bat"), "tool");
    assert_eq!(program_stem("node"), "node");
    assert_eq!(program_stem("archive.tar"), "archive.tar", "only an executable's extension goes");
    let r = Registry::load(&[]);
    let none = || None;
    let line = |s: &'static str| move || Some(s.to_string());
    let named = |p: &str, l: Option<&'static str>| match l {
        Some(l) => r.of_process(p, line(l)).map(|d| d.name.clone()),
        None => r.of_process(p, none).map(|d| d.name.clone()),
    };
    assert_eq!(named("codew.exe", None).as_deref(), Some("codewhale"));
    assert_eq!(named(r"C:\npm\CODEWHALE.cmd", None).as_deref(), Some("codewhale"));
    assert_eq!(
        named("node", Some(r"node C:\npm\node_modules\codewhale\bin\codewhale.js")).as_deref(),
        Some("codewhale")
    );
    assert_eq!(named("cmd.exe", Some(r#"cmd.exe /c "C:\npm\codewhale.cmd""#)).as_deref(), Some("codewhale"));
    assert_eq!(named("node", Some("node server.js")), None);
    assert_eq!(named("node", None), None, "a command line not read: not it");
    assert_eq!(named("pwsh", Some("pwsh -c codewhale")), None, "only a node, bun, deno or cmd is looked into");
}

#[test]
fn a_hooks_input_is_taken_where_the_definition_says() {
    let input: Value = serde_json::json!({ "session": { "id": "s-1" }, "turn": 3, "bad": "a\u{7}b", "list": [1] });
    let env = |k: &str| match k {
        "SID" => Some(" sess_9 ".to_string()),
        "LONG" => Some("x".repeat(5000)),
        _ => None,
    };
    assert_eq!(Def::take("/session/id", Some(&input), &env).as_deref(), Some("s-1"));
    assert_eq!(Def::take("/turn", Some(&input), &env).as_deref(), Some("3"));
    assert_eq!(Def::take("env:SID", None, &env).as_deref(), Some("sess_9"), "trimmed");
    assert_eq!(Def::take("/bad", Some(&input), &env), None, "a control character");
    assert_eq!(Def::take("/list", Some(&input), &env), None, "not a word");
    assert_eq!(Def::take("/missing", Some(&input), &env), None);
    assert_eq!(Def::take("/session/id", None, &env), None, "no input");
    assert_eq!(Def::take("env:LONG", None, &env), None, "longer than a path");
    assert_eq!(Def::take("env:UNSET", None, &env), None);
}

#[test]
fn the_agents_directory_comes_from_its_variable_else_home() {
    let d = parse(&base_with("dir = \"~/.toy\"", "dir = \"~/.toy\"\ndir_env = \"TOY_HOME\""), "t").unwrap();
    let home = Path::new("/home/me");
    assert_eq!(d.dir(home, &|_| None), Some(home.join(".toy")));
    assert_eq!(d.dir(home, &|_| Some(String::new())), Some(home.join(".toy")), "set but empty: not it");
    assert_eq!(d.dir(home, &|k| (k == "TOY_HOME").then(|| "/elsewhere".into())), Some(PathBuf::from("/elsewhere")));
}

/// Each rule, broken once: the file is refused, and the reason names what.
#[test]
fn a_definition_that_breaks_a_rule_is_refused_saying_why() {
    assert!(parse(BASE, "t").is_ok(), "{:?}", parse(BASE, "t").err());
    let cases: &[(&str, &str, &str)] = &[
        ("name = \"toy\"", "name = \"toY\"", "lower-case letters"),
        ("name = \"toy\"", "name = \"to_y\"", "lower-case letters"),
        ("name = \"toy\"", "name = \"1toy\"", "starting with a letter"),
        ("format = 1", "format = 2", "format 2"),
        ("programs = [\"toy\"]", "programs = []", "at least one program"),
        ("programs = [\"toy\"]", "programs = [\"bin/toy\"]", "no path"),
        ("programs = [\"toy\"]", "programs = [\"toy\", \"pwsh\"]", "a runner, a shell"),
        ("programs = [\"toy\"]", "programs = [\"NODE.exe\"]", "a runner, a shell"),
        ("programs = [\"toy\"]", "programs = [\"claude\"]", "keepane's own agents"),
        ("programs = [\"toy\"]", "programs = [\"cursor-agent\"]", "keepane's own agents"),
        ("command_line = [\"toy-agent\"]", "command_line = [\"ab\"]", "at least 3 characters"),
        ("ready = [\"idle\"]", "ready = []", "events.ready"),
        ("busy = [\"busy\"]", "busy = [\"busy\"]\nnope = [\"x\"]", "unknown field"),
        ("[setup]", "[hook_input]\nsession = \"session_id\"\n\n[setup]", "hook_input.session"),
        ("[setup]", "[hook_input]\nmodel = \"env:A-B\"\n\n[setup]", "hook_input.model"),
        ("format = 1", "format = 1\nresume = \"toy --resume\"", "{id}"),
        ("path = \"hooks.toml\"", "path = \"../hooks.toml\"", "no .."),
        ("path = \"hooks.toml\"", "path = \"/etc/hooks.toml\"", "no .."),
        ("path = \"hooks.toml\"", "path = \"C:/hooks.toml\"", "no .."),
        ("mark = \"keepane pane-ready\"\n", "", "needs text and mark"),
        ("mark = \"keepane pane-ready\"", "mark = \"something else\"", "does not hold its mark"),
        ("busy = \"{busy}\"", "busy = \"{step_start}\"", "not one of"),
        ("busy = [\"busy\"]", "wait = [\"w\"]", "events.busy is empty"),
        ("head_mark = \"[hooks]\"\n", "", "go together"),
        ("head_mark = \"[hooks]\"", "head_mark = \"[other]\"", "does not hold its head_mark"),
        ("how = \"append\"", "how = \"copy\"", "unknown variant"),
        ("dir = \"~/.toy\"", "dir_env = \"TOY HOME\"", "dir_env"),
    ];
    for (from, to, why) in cases {
        let text = base_with(from, to);
        match parse(&text, "t") {
            Ok(_) => panic!("taken, but should be refused for {why:?}:\n{text}"),
            Err(e) => assert!(e.contains(why), "refused for {e:?}, not {why:?}"),
        }
    }
    // json-merge: a place and a value, the place a pointer.
    let merge = "[[setup.files]]\npath = \"mcp.json\"\nhow = \"json-merge\"\n";
    let with = |extra: &str| format!("{BASE}\n{merge}{extra}");
    assert!(parse(&with("at = \"/servers/keepane\"\nvalue = { command = \"keepane\" }\n"), "t").is_ok());
    assert!(parse(&with("value = 1\n"), "t").unwrap_err().contains("needs at and value"));
    assert!(parse(&with("at = \"servers\"\nvalue = 1\n"), "t").unwrap_err().contains("JSON pointer"));
    assert!(parse(&with("at = \"/s\"\nvalue = 1\nmark = \"m\"\n"), "t").unwrap_err().contains("are for append"));
    // Too many of something.
    let many = format!("[{}]", (0..33).map(|i| format!("\"p{i}\"")).collect::<Vec<_>>().join(","));
    assert!(
        parse(&base_with("programs = [\"toy\"]", &format!("programs = {many}")), "t")
            .unwrap_err()
            .contains("at most 32")
    );
}

/// A person's directory: a good file loads; a broken one, one taking a
/// name keepane has or one already taken, are skipped and said; other
/// files are not looked at.
#[test]
fn a_directory_of_definitions_loads_what_is_right() {
    let dir = std::env::temp_dir().join(format!("keepane-agentdef-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a-toy.toml"), BASE).unwrap();
    std::fs::write(dir.join("b-broken.toml"), "name = ").unwrap();
    std::fs::write(dir.join("c-claude.toml"), base_with("name = \"toy\"", "name = \"claude\"")).unwrap();
    std::fs::write(dir.join("d-again.toml"), BASE).unwrap();
    std::fs::write(dir.join("e-codewhale.toml"), base_with("name = \"toy\"", "name = \"codewhale\"")).unwrap();
    std::fs::write(dir.join("f-big.toml"), "#".repeat(70 * 1024)).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a definition").unwrap();
    let r = Registry::load(std::slice::from_ref(&dir));
    let names: Vec<&str> = r.defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["codewhale", "toy"]);
    assert_eq!(r.errors.len(), 5, "{:#?}", r.errors);
    let said = |file: &str, why: &str| r.errors.iter().any(|e| e.contains(file) && e.contains(why));
    assert!(said("b-broken.toml", "b-broken.toml"), "{:#?}", r.errors);
    assert!(said("c-claude.toml", "keepane's own agent"), "{:#?}", r.errors);
    assert!(said("d-again.toml", "defined already"), "{:#?}", r.errors);
    assert!(said("e-codewhale.toml", "agents/codewhale.toml"), "{:#?}", r.errors);
    assert!(said("f-big.toml", "at most 65536"), "{:#?}", r.errors);
    // The same files, the same result: the order is the files' names.
    let again = Registry::load(std::slice::from_ref(&dir));
    assert_eq!(again.errors, r.errors);
    let _ = std::fs::remove_dir_all(&dir);
}
