# Browser setup and agent identity

An existing-profile browser task can prepare its exact PID and native window
through `dcc_cua_task_call` with method `browser_prepare`:

```json
{
  "task_id": "<task returned by start_task>",
  "method": "browser_prepare",
  "params": {
    "request": {
      "strategy": {"kind": "existing_profile"},
      "delivery_mode": "foreground"
    }
  }
}
```

Take a fresh observation on the same connection before preparation. Explicit
`foreground` delivery reaches the native driver; omitted delivery keeps the
`background` default. Unknown delivery modes fail before native dispatch.
The native adapter owns the exact-window remote-debugging setup and browser
consent handling. Preparation does not install or silently pair an extension.
Keep the connection alive for the complete response: a client-side wait ending
does not establish cancellation or failure of the underlying operation.

The control banner identifies the provider, MCP client and task:
`DCC-CUA · Codex · Fab plugin publication`. Its second line retains the live
activity, and Escape retains the existing interruption behavior. Client identity
comes from the first MCP `initialize` message: `clientInfo.title`, then
`clientInfo.name`, then `Agent`. The standard `codex-mcp-client` name displays as
`Codex`. Names are bounded and control/bidirectional override characters are
removed. Identity is presentation metadata and grants no authority.

Chrome's own automation infobar is browser-owned and retains Chrome's wording.
The DCC-CUA banner supplies the agent and task context without changing that
browser-owned surface.
