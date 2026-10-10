use email_ingestor::{config::Config, error::Kind, imap::Imap, smtp::Smtp, wire::Wire};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn config() -> Config {
    Config::parse(
        [
            ("IMAP_HOST", "imap.example"),
            ("IMAP_USERNAME", "user"),
            ("IMAP_PASSWORD", "password"),
            ("SMTP_HOST", "smtp.example"),
            ("SMTP_USERNAME", "user"),
            ("SMTP_PASSWORD", "password"),
            ("SMTP_RECIPIENT", "recipient@example.com"),
            ("DESTINATION_FOLDER", "Trash"),
        ]
        .into_iter()
        .map(|(key, value)| (format!("INGESTOR_{key}"), value.to_owned()))
        .collect(),
    )
    .unwrap()
}

fn scripted_wire(steps: Vec<(String, String)>) -> (Wire, tokio::task::JoinHandle<()>) {
    let (client, server) = tokio::io::duplex(4096);
    let peer = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        for (expected, reply) in steps {
            if !expected.is_empty() {
                let mut command = String::new();
                server.read_line(&mut command).await.unwrap();
                assert_eq!(command, expected);
            }
            server.get_mut().write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (Wire::new(client), peer)
}

#[tokio::test]
async fn rcpt_accepts_all_standard_success_codes() {
    for code in [250, 251, 252] {
        let (wire, peer) = scripted_wire(vec![
            (
                "MAIL FROM:<sender@example.com>\r\n".into(),
                "250 sender accepted\r\n".into(),
            ),
            (
                "RCPT TO:<recipient@example.com>\r\n".into(),
                format!("{code} recipient accepted\r\n"),
            ),
            ("DATA\r\n".into(), "354 send data\r\n".into()),
            ("Subject: test\r\n".into(), String::new()),
            ("\r\n".into(), String::new()),
            ("body\r\n".into(), String::new()),
            (".\r\n".into(), "250 accepted\r\n".into()),
        ]);
        let mut smtp = Smtp::from_wire(wire);
        smtp.start("sender@example.com", "recipient@example.com", 23)
            .await
            .unwrap();
        smtp.write(b"Subject: test\r\n\r\nbody\r\n").await.unwrap();
        smtp.finish().await.unwrap();
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn bare_smtp_replies_complete_submission() {
    let (wire, peer) = scripted_wire(vec![
        ("MAIL FROM:<>\r\n".into(), "250\r\n".into()),
        (
            "RCPT TO:<recipient@example.com>\r\n".into(),
            "250\r\n".into(),
        ),
        ("DATA\r\n".into(), "354\r\n".into()),
        ("\r\n".into(), String::new()),
        (".\r\n".into(), "250-first line\r\n250\r\n".into()),
    ]);
    let mut smtp = Smtp::from_wire(wire);
    smtp.start("", "recipient@example.com", 2).await.unwrap();
    smtp.write(b"\r\n").await.unwrap();
    smtp.finish().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn imap_rev2_builtin_move_is_supported() {
    let (wire, peer) = scripted_wire(vec![
        (String::new(), "* OK ready\r\n".into()),
        ("I1 AUTHENTICATE PLAIN\r\n".into(), "+ authenticate\r\n".into()),
        ("AHVzZXIAcGFzc3dvcmQ=\r\n".into(), "I1 OK authenticated\r\n".into()),
        ("I2 CAPABILITY\r\n".into(), "* CAPABILITY IMAP4rev2 AUTH=PLAIN\r\nI2 OK capabilities\r\n".into()),
        ("I3 SELECT \"Trash\"\r\n".into(), "* OK [UIDVALIDITY 10] generation\r\n* OK [UIDNEXT 1] next\r\nI3 OK [READ-WRITE] selected\r\n".into()),
        ("I4 SELECT \"INBOX\"\r\n".into(), "* OK [UIDVALIDITY 42] generation\r\n* OK [UIDNEXT 2] next\r\nI4 OK [READ-WRITE] selected\r\n".into()),
        ("I5 UID MOVE 1 \"Trash\"\r\n".into(), "* 1 EXPUNGE\r\nI5 OK moved\r\n".into()),
    ]);
    let mut imap = Imap::from_wire(wire);
    imap.setup(&config()).await.unwrap();
    assert_eq!(imap.generation, 42);
    imap.move_uid(1, "Trash").await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn rcpt_rejections_keep_their_failure_classification() {
    for (code, kind) in [(450, Kind::Temporary), (550, Kind::Fatal)] {
        let (wire, peer) = scripted_wire(vec![
            ("MAIL FROM:<>\r\n".into(), "250 ok\r\n".into()),
            (
                "RCPT TO:<recipient@example.com>\r\n".into(),
                format!("{code} rejected\r\n"),
            ),
        ]);
        let mut smtp = Smtp::from_wire(wire);
        assert_eq!(
            smtp.start("", "recipient@example.com", 2)
                .await
                .unwrap_err()
                .kind,
            kind
        );
        peer.await.unwrap();
    }
}
