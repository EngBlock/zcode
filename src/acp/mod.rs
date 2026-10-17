//! The Agent Client Protocol (ACP, agentclientprotocol.com): zcode as the
//! client of coding agents such as Claude Code, Codex and Gemini CLI, which
//! run as child processes speaking JSON-RPC on stdio. The protocol's types
//! come from the `agent-client-protocol-schema` crate; the transport
//! (`connection.rs`) is zcode's, like its language server client. The panel
//! that shows a conversation is `workspace/agent/`.

pub mod agents;
pub mod connection;
pub mod thread;

pub use agent_client_protocol_schema::v1 as schema;
pub use agents::AgentServer;
pub use connection::{AgentConnection, Incoming, Launch, RpcError};
pub use thread::{Change, Entry, Thread, ToolCallEntry, ToolContent, UserMessage};
