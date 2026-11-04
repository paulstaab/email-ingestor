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

fn smtp_wire(mut steps: Vec<(String, String)>) -> (Wire, tokio::task::JoinHandle<()>) {
    let mut setup = vec![
        (String::new(), "220 ready\r\n".into()),
        (
            "EHLO email-ingestor.invalid\r\n".into(),
            "250-peer\r\n250 AUTH PLAIN\r\n".into(),
        ),
        (
            "AUTH PLAIN AHVzZXIAcGFzc3dvcmQ=\r\n".into(),
            "235 authenticated\r\n".into(),
        ),
    ];
    setup.append(&mut steps);
    scripted_wire(setup)
}

#[tokio::test]
async fn rcpt_accepts_all_standard_success_codes() {
    for code in [250, 251, 252] {
        let (wire, peer) = smtp_wire(vec![
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
        smtp.setup(&config()).await.unwrap();
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
    let (wire, peer) = smtp_wire(vec![
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
    smtp.setup(&config()).await.unwrap();
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
        let (wire, peer) = smtp_wire(vec![
            ("MAIL FROM:<>\r\n".into(), "250 ok\r\n".into()),
            (
                "RCPT TO:<recipient@example.com>\r\n".into(),
                format!("{code} rejected\r\n"),
            ),
        ]);
        let mut smtp = Smtp::from_wire(wire);
        smtp.setup(&config()).await.unwrap();
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

#[tokio::test]
async fn smtp_rejects_submission_before_authentication_without_writing() {
    let (client, mut server) = tokio::io::duplex(4096);
    let mut smtp = Smtp::from_wire(Wire::new(client));
    assert_eq!(
        smtp.start("", "recipient@example.com", 2)
            .await
            .unwrap_err()
            .kind,
        Kind::Fatal
    );
    drop(smtp);
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut server, &mut bytes)
        .await
        .unwrap();
    assert!(bytes.is_empty());
}

// TC-10/49: the EOF probe may use a quoted empty string instead of a zero-byte literal.
#[tokio::test]
async fn imap_quoted_empty_eof_completes_retrieval() {
    let (wire, peer) = scripted_wire(vec![(
        "I1 UID FETCH 7 (UID BODY.PEEK[]<23.1048576>)\r\n".into(),
        "* 1 FETCH (UID 7 BODY[]<23> \"\")\r\nI1 OK complete\r\n".into(),
    )]);
    let mut imap = Imap::from_wire(wire);
    let result = imap.chunk(7, 23).await;
    peer.await.unwrap();
    assert_eq!(
        result.expect("a quoted empty EOF response must be accepted"),
        Vec::<u8>::new()
    );
}

// TC-06: rev2 uses ESEARCH, including when UID SEARCH has no RETURN options.
#[tokio::test]
async fn imap_rev2_esearch_discovers_source_uids() {
    let (wire, peer) = scripted_wire(vec![
        (String::new(), "* OK ready\r\n".into()),
        ("I1 AUTHENTICATE PLAIN\r\n".into(), "+ authenticate\r\n".into()),
        ("AHVzZXIAcGFzc3dvcmQ=\r\n".into(), "I1 OK authenticated\r\n".into()),
        ("I2 CAPABILITY\r\n".into(), "* CAPABILITY IMAP4rev2 AUTH=PLAIN\r\nI2 OK capabilities\r\n".into()),
        ("I3 SELECT \"Trash\"\r\n".into(), "* OK [UIDVALIDITY 10] generation\r\n* OK [UIDNEXT 1] next\r\nI3 OK [READ-WRITE] selected\r\n".into()),
        ("I4 SELECT \"INBOX\"\r\n".into(), "* OK [UIDVALIDITY 42] generation\r\n* OK [UIDNEXT 101] next\r\nI4 OK [READ-WRITE] selected\r\n".into()),
        ("I5 UID SEARCH UID 1:100\r\n".into(), "* ESEARCH (TAG \"I5\") UID ALL 2,7:9,99\r\nI5 OK complete\r\n".into()),
    ]);
    let mut imap = Imap::from_wire(wire);
    imap.setup(&config()).await.unwrap();
    let result = imap.search(1, 100).await;
    peer.await.unwrap();
    assert_eq!(
        result.expect("rev2 ESEARCH results must be discovered"),
        [2, 7, 8, 9, 99]
    );
}

// TC-05: advertising both revisions leaves rev1 behavior active until ENABLE.
#[tokio::test]
async fn imap_dual_revision_requires_enable_before_utf8_mailboxes() {
    let mut config = config();
    config.destination = "台北".into();
    let (client, server) = tokio::io::duplex(4096);
    let peer = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        server.get_mut().write_all(b"* OK ready\r\n").await.unwrap();
        for (expected, reply) in [
            ("I1 AUTHENTICATE PLAIN\r\n", "+ authenticate\r\n"),
            ("AHVzZXIAcGFzc3dvcmQ=\r\n", "I1 OK authenticated\r\n"),
            (
                "I2 CAPABILITY\r\n",
                "* CAPABILITY IMAP4rev1 IMAP4rev2 ENABLE MOVE AUTH=PLAIN\r\nI2 OK capabilities\r\n",
            ),
        ] {
            let mut line = String::new();
            server.read_line(&mut line).await.unwrap();
            assert_eq!(line, expected);
            server.get_mut().write_all(reply.as_bytes()).await.unwrap();
        }
        let mut command = String::new();
        server.read_line(&mut command).await.unwrap();
        // Both retaining rev1 encoding and explicitly enabling rev2 are valid fixes.
        let utf8_enabled = command == "I3 ENABLE IMAP4rev2\r\n";
        if utf8_enabled {
            server
                .get_mut()
                .write_all(b"* ENABLED IMAP4rev2\r\nI3 OK enabled\r\n")
                .await
                .unwrap();
            command.clear();
            server.read_line(&mut command).await.unwrap();
        }
        let (tag, _) = command.split_once(' ').unwrap();
        let expected = if utf8_enabled { "台北" } else { "&U,BTFw-" };
        let valid = command == format!("{tag} SELECT \"{expected}\"\r\n");
        if !valid {
            server
                .get_mut()
                .write_all(format!("{tag} NO mailbox encoding requires ENABLE\r\n").as_bytes())
                .await
                .unwrap();
            return command;
        }
        server.get_mut().write_all(format!("* OK [UIDVALIDITY 10] generation\r\n* OK [UIDNEXT 1] next\r\n{tag} OK [READ-WRITE] selected\r\n").as_bytes()).await.unwrap();
        command.clear();
        server.read_line(&mut command).await.unwrap();
        let (tag, _) = command.split_once(' ').unwrap();
        assert_eq!(command, format!("{tag} SELECT \"INBOX\"\r\n"));
        server.get_mut().write_all(format!("* OK [UIDVALIDITY 42] generation\r\n* OK [UIDNEXT 2] next\r\n{tag} OK [READ-WRITE] selected\r\n").as_bytes()).await.unwrap();
        command
    });
    let mut imap = Imap::from_wire(Wire::new(client));
    let result = imap.setup(&config).await;
    let last_command = peer.await.unwrap();
    assert!(
        result.is_ok(),
        "mailbox encoding must match the enabled revision; last command: {last_command:?}; result: {result:?}"
    );
    assert_eq!(imap.generation, 42);
}

#[tokio::test]
async fn imap_quoted_empty_eof_requires_identity_and_tagged_completion() {
    for response in [
        "* 1 FETCH (UID 8 BODY[]<23> \"\")\r\nI1 OK complete\r\n",
        "* 1 FETCH (UID 7 BODY[]<24> \"\")\r\nI1 OK complete\r\n",
        "* 1 FETCH (BODY[]<23> \"\" UID 8)\r\nI1 OK complete\r\n",
        "* 1 FETCH (BODY[]<23> \"\")\r\nI1 OK complete\r\n",
        "* 1 FETCH (UID 7 BODY[]<23> \"\")\r\nI1 NO rejected\r\n",
        "* 1 FETCH (UID 7 BODY[]<23> \"\")\r\n",
    ] {
        let (wire, peer) = scripted_wire(vec![(
            "I1 UID FETCH 7 (UID BODY.PEEK[]<23.1048576>)\r\n".into(),
            response.into(),
        )]);
        let mut imap = Imap::from_wire(wire);
        let result = imap.chunk(7, 23).await;
        peer.await.unwrap();
        assert!(
            result.is_err(),
            "invalid EOF response accepted: {response:?}"
        );
    }
}
