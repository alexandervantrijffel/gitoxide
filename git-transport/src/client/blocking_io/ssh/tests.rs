mod options {
    mod ssh_command {
        use crate::client::ssh::connect::Options;
        use crate::client::ssh::ProgramKind;

        #[test]
        fn no_field_means_ssh() {
            assert_eq!(Options::default().ssh_command(), "ssh");
        }

        #[test]
        fn command_field_determines_ssh_command() {
            assert_eq!(
                Options {
                    command: Some("field-value".into()),
                    ..Default::default()
                }
                .ssh_command(),
                "field-value"
            );
            assert_eq!(
                Options {
                    command: Some("field-value".into()),
                    kind: Some(ProgramKind::TortoisePlink),
                    ..Default::default()
                }
                .ssh_command(),
                "field-value"
            );
        }

        #[test]
        fn kind_serves_as_fallback() {
            assert_eq!(
                Options {
                    kind: Some(ProgramKind::TortoisePlink),
                    ..Default::default()
                }
                .ssh_command(),
                "tortoiseplink.exe"
            );
        }
    }
}

mod program_kind {
    mod from_os_str {
        use crate::client::ssh::ProgramKind;
        use std::ffi::OsStr;

        #[test]
        fn known_variants_are_derived_from_basename() {
            for name_or_path in [
                "ssh",
                "ssh.exe",
                "SSH",
                "SSH.exe",
                "/bin/ssh",
                "/bin/SSH",
                #[cfg(windows)]
                "c:\\bin\\ssh.exe",
            ] {
                assert_eq!(
                    ProgramKind::from(OsStr::new(name_or_path)),
                    ProgramKind::Ssh,
                    "{name_or_path:?} could not be identified correctly"
                );
            }
            assert_eq!(
                ProgramKind::from(OsStr::new("TortoisePlink.exe")),
                ProgramKind::TortoisePlink
            );
            assert_eq!(ProgramKind::from(OsStr::new("putty")), ProgramKind::Putty);
            assert_eq!(
                ProgramKind::from(OsStr::new("../relative/Plink.exe")),
                ProgramKind::Plink
            );
        }

        #[test]
        fn unknown_variants_fallback_to_simple() {
            assert_eq!(
                ProgramKind::from(OsStr::new("something-unknown-that-does-not-exist-for-sure-foobar")),
                ProgramKind::Simple,
                "in theory, we could fail right here but we don't and leave non-existing programs to fail during handshake"
            );
        }

        #[test]
        fn ssh_disguised_within_a_script_cannot_be_detected_due_to_invocation_with_dash_g() {
            assert_eq!(
                ProgramKind::from(OsStr::new("ssh -VVV")),
                ProgramKind::Simple,
                "we don't execute the command here but assume simple, even though we could determine it's ssh if we would do what git does here"
            );
        }
    }

    mod prepare_invocation {
        use crate::client::ssh;
        use crate::client::ssh::ProgramKind;
        use crate::Protocol;
        use std::ffi::OsStr;

        #[test]
        fn ssh() {
            for (url, protocol, expected) in [
                ("ssh://user@host:42/p", Protocol::V1, &["ssh", "-p42", "user@host"][..]),
                ("ssh://user@host/p", Protocol::V1, &["ssh", "user@host"][..]),
                ("ssh://host/p", Protocol::V1, &["ssh", "host"][..]),
                (
                    "ssh://user@host:42/p",
                    Protocol::V2,
                    &["ssh", "-o", "SendEnv=GIT_PROTOCOL", "-p42", "user@host"][..],
                ),
                (
                    "ssh://user@host/p",
                    Protocol::V2,
                    &["ssh", "-o", "SendEnv=GIT_PROTOCOL", "user@host"][..],
                ),
                (
                    "ssh://host/p",
                    Protocol::V2,
                    &["ssh", "-o", "SendEnv=GIT_PROTOCOL", "host"][..],
                ),
            ] {
                assert_eq!(call_args(ProgramKind::Ssh, url, protocol), quoted(expected));
            }
        }

        #[test]
        fn tortoise_plink_has_batch_command() {
            assert_eq!(
                call_args(ProgramKind::TortoisePlink, "ssh://user@host:42/p", Protocol::V2),
                quoted(&["tortoiseplink.exe", "-batch", "-P42", "user@host"])
            );
        }

        #[test]
        fn port_for_all() {
            for kind in [ProgramKind::TortoisePlink, ProgramKind::Plink, ProgramKind::Putty] {
                assert!(call_args(kind, "ssh://user@host:43/p", Protocol::V2).ends_with(r#""-P43" "user@host""#));
            }
        }

        #[test]
        fn simple_cannot_handle_any_arguments() {
            match try_call(ProgramKind::Simple, "ssh://user@host:42/p", Protocol::V2) {
                Err(ssh::invocation::Error { .. }) => {}
                _ => panic!("BUG: unexpected outcome"),
            }
            assert_eq!(
                call_args(ProgramKind::Simple, "ssh://user@host/p", Protocol::V2),
                quoted(&["simple", "user@host"]),
                "simple can only do simple invocations"
            );
        }

        #[test]
        fn ssh_env_v2() {
            let prepare = call(ProgramKind::Ssh, "ssh://host/p", Protocol::V2);
            assert_eq!(
                prepare.env,
                &[
                    ("GIT_PROTOCOL".into(), "version=2".into()),
                    ("LANG".into(), "C".into()),
                    ("LC_ALL".into(), "C".into())
                ]
            );
            assert!(!prepare.use_shell);
        }

        #[test]
        fn disallow_shell_is_honored() -> Result {
            let url = git_url::parse("ssh://host/path".into()).expect("valid url");

            let disallow_shell = false;
            let prepare =
                ProgramKind::Ssh.prepare_invocation(OsStr::new("echo hi"), &url, Protocol::V1, disallow_shell)?;
            assert!(prepare.use_shell, "shells are used when needed");

            let disallow_shell = true;
            let prepare =
                ProgramKind::Ssh.prepare_invocation(OsStr::new("echo hi"), &url, Protocol::V1, disallow_shell)?;
            assert!(
                !prepare.use_shell,
                "but we can enforce it not to be used as well for historical reasons"
            );
            Ok(())
        }

        fn quoted(input: &[&str]) -> String {
            input.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(" ")
        }
        fn try_call(
            kind: ProgramKind,
            url: &str,
            version: Protocol,
        ) -> std::result::Result<git_command::Prepare, ssh::invocation::Error> {
            let ssh_cmd = kind.exe().unwrap_or_else(|| OsStr::new("simple"));
            let url = git_url::parse(url.into()).expect("valid url");
            kind.prepare_invocation(ssh_cmd, &url, version, false)
        }
        fn call(kind: ProgramKind, url: &str, version: Protocol) -> git_command::Prepare {
            try_call(kind, url, version).expect("no error")
        }
        fn call_args(kind: ProgramKind, url: &str, version: Protocol) -> String {
            format!("{:?}", std::process::Command::from(call(kind, url, version)))
        }

        type Result = std::result::Result<(), ssh::invocation::Error>;
    }

    mod line_to_permission_err {
        use crate::client::ssh::ProgramKind;

        #[test]
        fn ssh() {
            assert_eq!(
                ProgramKind::Ssh
                    .line_to_permission_err("byron@github.com: Permission denied (publickey).".into())
                    .map(|err| err.kind()),
                Ok(std::io::ErrorKind::PermissionDenied)
            );
        }

        #[test]
        fn simple() {
            assert_eq!(
                ProgramKind::Simple
                    .line_to_permission_err("something something permission denied something".into())
                    .map(|err| err.kind()),
                Ok(std::io::ErrorKind::PermissionDenied),
                "this kind is basically unknown but we try our best"
            );
        }

        #[test]
        fn tortoiseplink_putty_plink() {
            for kind in [ProgramKind::TortoisePlink, ProgramKind::Plink, ProgramKind::Putty] {
                assert_eq!(
                    kind
                        .line_to_permission_err("publickey".into())
                        .map(|err| err.kind()),
                    Ok(std::io::ErrorKind::PermissionDenied),
                    "this program pops up error messages in a window, no way to extract information from it. Maybe there is other ways to use it, 'publickey' they mention all"
                );
            }
        }
    }
}