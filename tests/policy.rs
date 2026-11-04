use email_ingestor::{
    HEADER_LIMIT,
    config::Config,
    envelope::{EnvelopeParser, Framer, address},
    error::{Error, Kind},
    supervisor::Streak,
};
use std::collections::HashMap;

fn values() -> HashMap<String, String> {
    [
        ("IMAP_HOST", "imap.example"),
        ("IMAP_USERNAME", "test-user"),
        ("IMAP_PASSWORD", "test-password"),
        ("SMTP_HOST", "smtp.example"),
        ("SMTP_USERNAME", "test-user"),
        ("SMTP_PASSWORD", "test-password"),
        ("SMTP_RECIPIENT", "recipient@example.com"),
        ("DESTINATION_FOLDER", "Trash"),
    ]
    .into_iter()
    .map(|(k, v)| (format!("INGESTOR_{k}"), v.to_owned()))
    .collect()
}
#[test]
fn configuration_contract() {
    let config = Config::parse(values()).unwrap();
    assert_eq!(config.imap.port, 993);
    assert_eq!(config.smtp.port, 465);
    assert_eq!(config.max_bytes, 262_144_000);
    assert_eq!(config.source, "INBOX");
    for (key, value) in [
        ("UNKNOWN", "x"),
        ("IMAP_HOST", ""),
        ("SMTP_PORT", "0"),
        ("SMTP_PORT", "65536"),
        ("MAX_MESSAGE_BYTES", "0"),
        ("MAX_MESSAGE_BYTES", "-1"),
        ("LOG_LEVEL", "info,wire=trace"),
        ("DESTINATION_FOLDER", "inbox"),
        ("SMTP_RECIPIENT", "a@b\r\nRCPT TO:<x@y>"),
        ("IMAP_PASSWORD_FILE", "/missing"),
        ("SMTP_CA_FILE", "/missing"),
    ] {
        let mut env = values();
        env.insert(format!("INGESTOR_{key}"), value.to_owned());
        assert!(Config::parse(env).is_err(), "{key} should fail");
    }
    let mut env = values();
    env.insert("UNRELATED".into(), "ignored".into());
    assert!(Config::parse(env).is_ok());
}
#[test]
fn envelope_policy_across_every_boundary() {
    let raw =
        b"Return-Path:\r\n <sender@example.com>\r\nFrom: unrelated@example.com\r\n\r\nbody\r\n";
    for split in 0..=raw.len() {
        let mut parser = EnvelopeParser::default();
        parser.feed(&raw[..split]).unwrap();
        parser.feed(&raw[split..]).unwrap();
        assert_eq!(parser.sender().unwrap(), "sender@example.com");
    }
    for raw in [
        "From: x@y\r\n\r\n",
        "Return-Path: <>\r\n\r\n",
        "Return-Path: <x@y>\r\nReturn-Path: <x@y>\r\n\r\n",
        "Return-Path: <bad>\r\n\r\n",
        "Return-Path: <x@y>\r\n evil\r\n\r\n",
    ] {
        let mut parser = EnvelopeParser::default();
        parser.feed(raw.as_bytes()).unwrap();
        assert_eq!(parser.sender().unwrap(), "");
    }
    let mut parser = EnvelopeParser::default();
    assert!(parser.feed(&vec![b'x'; HEADER_LIMIT + 1]).is_err());
    let mut parser = EnvelopeParser::default();
    parser.feed(b"Return-Path: <x@y>\r\n").unwrap();
    assert!(parser.sender().is_err());
}
#[test]
fn faithful_framing_at_every_boundary() {
    let raw = b"Header: unchanged\r\n\r\n.leading\r\n..two\r\nlast\r\n";
    let expected = b"Header: unchanged\r\n\r\n..leading\r\n...two\r\nlast\r\n";
    for split in 0..=raw.len() {
        let mut framer = Framer::new(false);
        let mut output = Vec::new();
        let mut all = Vec::new();
        framer.encode(&raw[..split], &mut output).unwrap();
        all.extend_from_slice(&output);
        framer.encode(&raw[split..], &mut output).unwrap();
        all.extend_from_slice(&output);
        framer.finish().unwrap();
        assert_eq!(all, expected);
    }
    for input in [b"bare\n".as_slice(), b"bare\rx", b"nul\0\r\n", b"\xff\r\n"] {
        assert!(Framer::new(false).encode(input, &mut Vec::new()).is_err());
    }
    for input in [b"".as_slice(), b"no final newline", b"partial\r"] {
        let mut framer = Framer::new(true);
        framer.encode(input, &mut Vec::new()).unwrap();
        assert!(framer.finish().is_err());
    }
    assert!(
        Framer::new(true)
            .encode(&vec![b'x'; 999], &mut Vec::new())
            .is_err()
    );
    let mut framer = Framer::new(true);
    framer.encode(b"\xff\r\n", &mut Vec::new()).unwrap();
    framer.finish().unwrap();
}
#[test]
fn address_policy_prevents_command_injection() {
    for value in [
        "x@example.com",
        "\"a b\"@example.com",
        "x@[127.0.0.1]",
        "x@[IPv6:::1]",
        "ü@example.com",
    ] {
        assert!(address(value), "{value}");
    }
    for value in [
        "Display <x@y>",
        "x@y,x@z",
        "x@y\r\nMAIL FROM:<a@b>",
        "x@-host",
        ".x@y",
        "x..y@z",
        "x@",
        "@y",
    ] {
        assert!(!address(value), "{value}");
    }
}
#[test]
fn independent_failure_streams_stop_at_five() {
    let mut affected = Streak::default();
    let mut unrelated = Streak::default();
    for attempt in 1..=4 {
        let delay = affected
            .failure(Error::temporary("fixture"), "fetch")
            .unwrap();
        assert!(delay.as_secs() >= 5 << (attempt - 1));
        assert!(delay.as_secs() <= (5 << (attempt - 1)) + 1);
        unrelated.success("smtp_setup");
        assert_eq!(affected.failures(), attempt);
    }
    assert_eq!(
        affected
            .failure(Error::temporary("fixture"), "fetch")
            .unwrap_err()
            .kind,
        Kind::Fatal
    );
    assert_eq!(affected.failures(), 5);
    let mut streak = Streak::default();
    assert!(streak.failure(Error::fatal("fixture"), "fetch").is_err());
    assert_eq!(streak.failures(), 0);
    streak
        .failure(Error::temporary("fixture"), "fetch")
        .unwrap();
    streak.success("fetch");
    assert_eq!(streak.failures(), 0);
}

#[test]
fn smtp_security_contract() {
    assert_eq!(
        Config::parse(values()).unwrap().smtp.security.name(),
        "implicit-tls"
    );
    for host in ["127.0.0.1", "127.255.1.2", "::1"] {
        let mut env = values();
        env.insert("INGESTOR_SMTP_SECURITY".into(), "plaintext-loopback".into());
        env.insert("INGESTOR_SMTP_HOST".into(), host.into());
        let config = Config::parse(env.clone()).unwrap();
        assert_eq!(config.smtp.port, 465);
        assert_eq!(config.smtp.security.name(), "plaintext-loopback");
        for ca in ["", "/missing"] {
            let mut conflicting = env.clone();
            conflicting.insert("INGESTOR_SMTP_CA_FILE".into(), ca.into());
            assert!(Config::parse(conflicting).is_err());
        }
        for key in ["SMTP_USERNAME", "SMTP_PASSWORD"] {
            let mut missing = env.clone();
            missing.remove(&format!("INGESTOR_{key}"));
            assert!(Config::parse(missing).is_err());
        }
    }
    for host in [
        "localhost",
        "smtp.example",
        "0.0.0.0",
        "::",
        "192.168.1.1",
        "::ffff:127.0.0.1",
        "::ffff:7f00:1",
        "[::1]",
        "127.1",
        "",
        "::1%lo",
    ] {
        let mut env = values();
        env.insert("INGESTOR_SMTP_SECURITY".into(), "plaintext-loopback".into());
        env.insert("INGESTOR_SMTP_HOST".into(), host.into());
        assert!(Config::parse(env).is_err(), "{host}");
    }
    for mode in ["", "plaintext", "IMPLICIT-TLS", "starttls", " implicit-tls"] {
        let mut env = values();
        env.insert("INGESTOR_SMTP_SECURITY".into(), mode.into());
        assert!(Config::parse(env).is_err());
    }
    let mut env = values();
    env.insert("INGESTOR_SMTP_SECURITY".into(), "implicit-tls".into());
    assert!(Config::parse(env).is_ok());
}
