use crate::client::Kind;

/// Something the user can do with one key; the keybindings menu lists and runs these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NextPanel,
    PrevPanel,
    BackToList,
    Send,
    Cancel,
    ToggleKind,
    ToggleRequestTab,
    NextMethod,
    PrevMethod,
    PickMethod,
    LoadSchema,
    ToggleSchemaSource,
    ToggleTls,
    OpenLog,
    Quit,
}

/// One row of the keybindings menu.
#[derive(Debug, Clone, Copy)]
pub struct Command {
    pub key: &'static str,
    pub description: &'static str,
    pub action: Action,
    /// `None`: both modes.
    pub only: Option<Kind>,
}

const fn cmd(
    key: &'static str,
    description: &'static str,
    action: Action,
    only: Option<Kind>,
) -> Command {
    Command {
        key,
        description,
        action,
        only,
    }
}

pub const COMMANDS: &[Command] = &[
    cmd("<tab>", "Focus next panel", Action::NextPanel, None),
    cmd(
        "<shift+tab>",
        "Focus previous panel",
        Action::PrevPanel,
        None,
    ),
    cmd(
        "<esc>",
        "Back to the request list",
        Action::BackToList,
        None,
    ),
    cmd("<ctrl+s>", "Send the request", Action::Send, None),
    cmd(
        "<ctrl+x>",
        "Cancel the call in flight",
        Action::Cancel,
        None,
    ),
    cmd(
        "<ctrl+g>",
        "Switch between REST and gRPC",
        Action::ToggleKind,
        None,
    ),
    cmd(
        "<ctrl+t>",
        "Switch the request box between body and headers",
        Action::ToggleRequestTab,
        None,
    ),
    cmd(
        "<ctrl+n>",
        "Next HTTP method",
        Action::NextMethod,
        Some(Kind::Http),
    ),
    cmd(
        "<ctrl+p>",
        "Previous HTTP method",
        Action::PrevMethod,
        Some(Kind::Http),
    ),
    cmd(
        "<ctrl+n>",
        "Pick the gRPC method",
        Action::PickMethod,
        Some(Kind::Grpc),
    ),
    cmd(
        "<ctrl+l>",
        "Load the schema",
        Action::LoadSchema,
        Some(Kind::Grpc),
    ),
    cmd(
        "<ctrl+o>",
        "Switch the schema source (reflection or file)",
        Action::ToggleSchemaSource,
        Some(Kind::Grpc),
    ),
    cmd(
        "<ctrl+k>",
        "Toggle TLS certificate verification",
        Action::ToggleTls,
        Some(Kind::Grpc),
    ),
    cmd("<ctrl+e>", "Open the log", Action::OpenLog, None),
    cmd("<ctrl+c>", "Quit", Action::Quit, None),
];

/// The commands that apply to `kind`, in menu order.
pub fn commands_for(kind: Kind) -> Vec<&'static Command> {
    COMMANDS
        .iter()
        .filter(|c| c.only.is_none_or(|k| k == kind))
        .collect()
}
