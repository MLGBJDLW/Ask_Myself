# MCP Connectors

MCP is Nexa's first external ecosystem lane. An MCP connector gives Nexa access
to tools exposed by an external process or remote service through the Model
Context Protocol.

MCP connectors are not native plugins. They run outside Nexa's core runtime and
are mediated by the connector host, tool approval policy, source scope, and
transport lifecycle.

## Connector Lifecycle

1. Configure
   - Choose a transport: `stdio`, `streamable_http`, or legacy `sse`.
   - Provide launch command, URL, arguments, environment, or headers as needed.
   - Keep credentials in connector config fields designed for secrets.
2. Test
   - Start or connect to the server.
   - Discover the server-defined tools.
   - Surface connection errors without enabling the connector silently.
3. Enable
   - Register discovered tools as MCP connector runtime tools.
   - Keep the connector disabled by default until the user enables it.
4. Use
   - Tool calls are keyed by server and tool identity.
   - High-risk calls still pass through approval policy.
   - Returned content is treated as external or mixed-trust evidence unless a
     connector-specific contract says otherwise.
5. Disable or delete
   - Disabling removes the runtime tools from discovery.
   - Deleting removes the stored connector configuration.

## Trust Model

MCP tools cross a trust boundary. Nexa should assume an MCP connector can:

- read data from outside the local knowledge base
- perform network operations
- mutate remote systems
- return prompt-injection content
- expose tool schemas that change between sessions

The connector host must therefore keep:

- server/tool identity in approval keys
- connection status visible
- discovered tools inspectable
- disabled connectors undiscoverable by the agent
- credentials out of model-visible context
- remote content marked as external unless explicitly grounded

## Product Language

Use "MCP connector" in user-facing UI and docs.

Use "MCP server" only when referring to the protocol endpoint, process, or
server-defined tool schema. This keeps the ecosystem model clear:

- Connector: the Nexa-managed external integration.
- Server: the MCP process or remote endpoint behind the connector.
- Tool: a server-defined callable capability exposed through the connector.

## Current Implementation

The current runtime stores MCP endpoint configuration as `McpServer` records and
wraps discovered server tools with `mcp_tool` or `mcp__...` tool names. That
internal naming can remain while the product language moves to connectors.

Advanced users can maintain the versioned `~/.nexa/connectors/mcp.json` file
(`NEXA_HOME` can override the `.nexa` root). Settings -> Extensions -> MCP
Connectors exposes the resolved path plus explicit Open and Reload actions. The
file is a declarative configuration source; startup and explicit Reload validate
the whole document before materializing its connectors into the existing
runtime store. Existing `mcp-connectors.json` content in app data is copied once
when the new target is missing; the legacy file is retained for rollback.

```json
{
  "version": 1,
  "connectors": {
    "local-docs": {
      "name": "Local Docs",
      "transport": {
        "type": "stdio",
        "command": "npx",
        "args": ["-y", "@example/docs-mcp"],
        "env": {
          "DOCS_TOKEN": "${env:DOCS_TOKEN}"
        }
      }
    },
    "remote-docs": {
      "name": "Remote Docs",
      "transport": {
        "type": "streamable_http",
        "url": "https://example.com/mcp",
        "headers": {
          "Authorization": "Bearer ${env:DOCS_TOKEN}"
        }
      }
    }
  }
}
```

Connector ids use letters, numbers, `.`, `-`, or `_`. Secret-shaped environment
variables and headers must use `${env:VARIABLE}` references; resolved values are
never written to the JSON or SQLite projection. A new file connector is disabled
until the user explicitly enables it. Reload preserves activation only when the
normalized trust-relevant configuration is unchanged; name, command, arguments,
URL, environment, or headers changing resets it to disabled. Invalid JSON
retains the last valid runtime projection and reports the parser line and column.

The JSON file does not carry trust receipts, tool approvals, health state, or
native code. Those remain host-owned state. File-backed connectors are edited in
the JSON rather than through the managed form.

Nexa's generic capability package loader can already read connector manifests
from `.nexa/capabilities/*/capability.yaml`. The next runtime step is to attach
that connector package metadata to saved server configs:

```yaml
id: github-mcp
name: GitHub
surface: connector
transport: streamable_http
permissions:
  read: true
  write: true
  network: true
```

Connector package metadata should describe setup, required credentials,
permissions, health checks, and safe default state. It should not load native
code into Nexa core.
