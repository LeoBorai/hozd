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
    NextBodyType,
    NewRequest,
    NextMethod,
    PrevMethod,
    PickMethod,
    LoadSchema,
    ToggleSchemaSource,
    ToggleTls,
    OpenLog,
    ToggleVariablesPanel,
    ToggleVars,
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
    cmd(
        "<ctrl+n>",
        "New request: clear the form",
        Action::NewRequest,
        None,
    ),
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
    cmd(
        "<ctrl+s>",
        "Send the request (WebSocket: connect, then send the body)",
        Action::Send,
        None,
    ),
    cmd(
        "<ctrl+x>",
        "Cancel the call in flight (WebSocket: close)",
        Action::Cancel,
        None,
    ),
    cmd(
        "<ctrl+g>",
        "Cycle REST, GraphQL, gRPC and WebSocket",
        Action::ToggleKind,
        None,
    ),
    cmd(
        "<ctrl+t>",
        "Switch the request box between body and headers (GraphQL: query, variables, headers)",
        Action::ToggleRequestTab,
        None,
    ),
    cmd(
        "<ctrl+y>",
        "Next body type: text, JSON, raw JSON, form, multipart, file",
        Action::NextBodyType,
        Some(Kind::Http),
    ),
    cmd(
        "<ctrl+p>",
        "Previous HTTP method",
        Action::PrevMethod,
        Some(Kind::Http),
    ),
    cmd(
        "<ctrl+p>",
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
        "<ctrl+p>",
        "Pick a query or mutation from the introspected schema",
        Action::PickMethod,
        Some(Kind::Graphql),
    ),
    cmd(
        "<ctrl+l>",
        "Introspect the endpoint for its schema",
        Action::LoadSchema,
        Some(Kind::Graphql),
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
    cmd(
        "<ctrl+v>",
        "Switch the left bottom panel between Logs and Variables",
        Action::ToggleVariablesPanel,
        None,
    ),
    cmd(
        "<ctrl+b>",
        "Toggle $variables in the request body",
        Action::ToggleVars,
        Some(Kind::Http),
    ),
    cmd("<ctrl+c>", "Quit", Action::Quit, None),
];

/// The commands that apply to `kind`, in menu order.
pub fn commands_for(kind: Kind) -> Vec<&'static Command> {
    COMMANDS
        .iter()
        .filter(|c| c.only.is_none_or(|k| k == kind))
        .collect()
}
