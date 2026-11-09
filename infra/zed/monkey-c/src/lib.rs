use zed_extension_api::{self as zed, LanguageServerId, Worktree};

struct MonkeyC;

impl zed::Extension for MonkeyC {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<zed::Command> {
        let nix = worktree
            .which("nix")
            .ok_or("Nix is required to start the repository-pinned Rafiki server")?;
        let root = worktree.root_path();
        let flake = format!(
            "git+file://{}#rafiki",
            root.replace('%', "%25")
                .replace(' ', "%20")
                .replace('#', "%23")
                .replace('?', "%3F")
        );
        Ok(zed::Command {
            command: nix,
            args: vec![
                "run".into(),
                "--no-update-lock-file".into(),
                flake,
                "--".into(),
                "server".into(),
            ],
            env: vec![],
        })
    }
}

zed::register_extension!(MonkeyC);
