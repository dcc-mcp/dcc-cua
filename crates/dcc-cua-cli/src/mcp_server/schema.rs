use super::*;

fn native_minimize_scope_schema() -> Value {
    json!({
        "title": "Observation-bound native window minimize",
        "type": "object",
        "additionalProperties": false,
        "required": ["action", "input_kind", "secret_input", "authorization_category"],
        "properties": {
            "action": {"const": "minimize_window"},
            "input_kind": {"const": "window_state"},
            "secret_input": {"const": false},
            "authorization_category": {"const": "window_state"},
            "browser_origin": {"type": "null"}
        }
    })
}

fn native_frame_scope_schema() -> Value {
    json!({
        "title":"Metadata-bound non-activating native window frame",
        "type":"object", "additionalProperties":false,
        "required":["action","input_kind","secret_input","authorization_category"],
        "properties":{
            "action":{"const":"set_window_frame"},"input_kind":{"const":"window_state"},
            "secret_input":{"const":false},"authorization_category":{"const":"window_state"},
            "browser_origin":{"type":"null"}
        }
    })
}

fn native_menu_dismiss_scope_schema() -> Value {
    json!({"title":"One-use native standard popup menu cancellation", "type":"object", "additionalProperties":false,
        "required":["action","input_kind","secret_input","authorization_category"],
        "properties":{"action":{"const":"dismiss_menu"},"input_kind":{"const":"window_state"},
            "secret_input":{"const":false},"authorization_category":{"const":"window_state"},"browser_origin":{"type":"null"}}})
}

pub(super) fn capture_preparation_scope_schema() -> Value {
    json!({
        "title":"Temporary passive exact-root capture preparation",
        "type":"object","additionalProperties":false,
        "required":["action","input_kind","secret_input","authorization_category"],
        "properties":{
            "action":{"const":"capture_preparation_begin"},"input_kind":{"const":"window_state"},
            "secret_input":{"const":false},"authorization_category":{"const":"window_state"},
            "browser_origin":{"type":"null"}
        }
    })
}

fn pixels_action_scope_schema() -> Value {
    json!({
        "oneOf": [native_minimize_scope_schema(), native_frame_scope_schema(), native_menu_dismiss_scope_schema(), capture_preparation_scope_schema(), recording_audio::scope_schema(), {
            "title": "Observation-bound foreground pixel input",
            "type": "object",
            "additionalProperties": false,
            "required": ["action", "input_kind", "secret_input", "authorization_category"],
            "properties": {
                "action": {"type": "string", "enum": TrustedTaskActionScope::PIXELS_INPUT_ACTIONS},
                "input_kind": {"const": "raw_input"},
                "secret_input": {"const": false},
                "authorization_category": {"const": "raw_input"},
                "browser_origin": {"type": "null"}
            }
        }]
    })
}

pub(super) fn task_action_scope_schema() -> Value {
    let required = [
        "action",
        "input_kind",
        "secret_input",
        "authorization_category",
    ];
    json!({
        "oneOf": [
            native_minimize_scope_schema(),
            native_frame_scope_schema(),
            native_menu_dismiss_scope_schema(),
            capture_preparation_scope_schema(),
            recording_audio::scope_schema(),
            {
                "title": "Exact-origin paired extension navigation",
                "type": "object", "additionalProperties": false,
                "required": ["action", "input_kind", "secret_input", "authorization_category", "browser_origin"],
                "properties": {
                    "action": {"const": "browser_extension_navigate"},
                    "input_kind": {"const": "browser"}, "secret_input": {"const": false},
                    "authorization_category": {"const": "external_effect"},
                    "browser_origin": {"type":"string", "minLength":1, "maxLength":2048, "pattern":"^https?://[^/?#@]+$"}
                }
            },
            {
                "title": "Semantic exact-window input",
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": TrustedTaskActionScope::NATIVE_ACTIONS.iter().copied().filter(|action| *action != "key_sequence").collect::<Vec<_>>(),
                        "description": "Final input action, not a Host method name. For example, browser_click resolves to click."
                    },
                    "input_kind": {"const": "semantic"},
                    "secret_input": {"type": "boolean"},
                    "authorization_category": {"type": "string", "enum": TrustedTaskActionScope::SEMANTIC_CATEGORIES},
                    "browser_origin": {"type": "null"}
                }
            },
            {
                "title": "Raw exact-window input",
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": {
                    "action": {"type": "string", "enum": TrustedTaskActionScope::NATIVE_ACTIONS},
                    "input_kind": {"const": "raw_input"},
                    "secret_input": {"type": "boolean"},
                    "authorization_category": {"type": "string", "enum": TrustedTaskActionScope::RAW_INPUT_CATEGORIES},
                    "browser_origin": {"type": "null"}
                }
            },
            {
                "title": "Browser credential input",
                "type": "object",
                "additionalProperties": false,
                "required": ["action", "input_kind", "secret_input", "authorization_category", "browser_origin"],
                "properties": {
                    "action": {"const": "browser_type"},
                    "input_kind": {"const": "browser"},
                    "secret_input": {"const": true},
                    "authorization_category": {"const": "credential"},
                    "browser_origin": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 2048,
                        "pattern": "^https?://[^/?#@]+$"
                    }
                }
            },
            {
                "title": "Clipboard credential capture",
                "type": "object",
                "additionalProperties": false,
                "required": required,
                "properties": {
                    "action": {"const": "clipboard_capture_secret"},
                    "input_kind": {"const": "clipboard"},
                    "secret_input": {"const": true},
                    "authorization_category": {"const": "credential"},
                    "browser_origin": {"type": "null"}
                }
            }
        ]
    })
}

pub(super) fn tool_definitions() -> Vec<Value> {
    let action_scope = task_action_scope_schema();
    let mut tools = vec![
        json!({
            "name": "start_task",
            "title": "Start DCC-CUA task",
            "description": "Start one exact bounded DCC-CUA task without a secondary confirmation step. Keep this mcp-server stdio connection open through stop_task: task IDs, observations and opaque refs are connection/session-scoped and cannot be resumed after reconnecting. The connected Agent Host owns user authorization. observation_mode defaults to semantic; explicitly choose pixels_only for an exact window snapshot without UIA or semantic selectors. Pixels-only tasks require complete exact-window capture proof and grant only closed native window_state methods or the advertised non-secret raw_input actions. Explicit set_window_frame requires get_window_state and its own action scope. Native dismiss_menu requires its independent window_state/dismiss_menu action scope plus get_window_state and change_window_state. These grant no raw input or recording. Capture does not require foreground activation. Every action requires the latest observation and an unchanged native instance. Explicitly granted semantic tasks may use background Windows UIA click, toggle, set_text or set_value on supported fresh same-task UIA elements; other semantic actions do not imply background support. Physical input requires the exact target in the foreground. Pointer locations must be unredacted and natively reachable; physical keyboard and text input require complete visibility. Explicit allow_recording=true grants native video recording_start/state/stop in an operator-owned task directory. Target playback audio additionally requires recording_audio mode/ceilings and its independent playback_audio action; silent video permission never enables sound. Live observation and recording reuse the same native source, without an accessibility tree or trajectory.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["application_label", "surface", "allowed_methods", "allowed_actions"],
                "oneOf": [
                    {
                        "required": ["target_process_id", "target_window_handle"],
                        "not": {"required": ["owned_browser_launch"]}
                    },
                    {
                        "required": ["owned_browser_launch"],
                        "not": {"anyOf": [
                            {"required": ["target_process_id"]},
                            {"required": ["target_window_handle"]}
                        ]}
                    }
                ],
                "allOf": [{
                    "if": {
                        "required": ["observation_mode"],
                        "properties": {"observation_mode": {"const": "pixels_only"}}
                    },
                    "then": {
                        "required": ["target_process_id", "target_window_handle"],
                        "not": {"required": ["owned_browser_launch"]},
                        "properties": {
                            "surface": {"const": "window"},
                            "allowed_browser_origins": {"maxItems": 0},
                            "allowed_methods": {"items": {"enum": [
                                "get_window_state", "change_window_state", "minimize_window", "set_window_frame", "snapshot",
                                "execute_action", "get_session_state", "get_input_state", "session_health", "poll_session_events",
                                "live_observation_start", "live_observation_state", "live_observation_stop",
                                "recording_start", "recording_state", "recording_stop"
                                ,"capture_preparation_begin", "capture_preparation_state", "capture_preparation_stop", "capture_preparation_snapshot"
                            ]}},
                            "allowed_actions": {"items": pixels_action_scope_schema()}
                        }
                    }
                }, {
                    "if":{"anyOf":[
                        {"required":["allow_capture_preparation"],"properties":{"allow_capture_preparation":{"const":true}}},
                        {"properties":{"allowed_methods":{"contains":{"enum":["capture_preparation_begin","capture_preparation_state","capture_preparation_stop","capture_preparation_snapshot"]}}}},
                        {"properties":{"allowed_actions":{"contains":capture_preparation_scope_schema()}}}
                    ]},
                    "then":{"required":["allow_capture_preparation","observation_mode"],"properties":{
                        "allow_capture_preparation":{"const":true},"observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"allOf":[
                            {"contains":{"const":"get_window_state"}}, {"contains":{"const":"capture_preparation_begin"}},
                            {"contains":{"const":"capture_preparation_state"}}, {"contains":{"const":"capture_preparation_stop"}}
                        ]}, "allowed_actions":{"contains":capture_preparation_scope_schema()}
                    }}
                }, {
                    "if": {"required":["allow_recording"],"properties":{"allow_recording":{"const":true}}},
                    "then": {"required":["observation_mode"],"properties":{
                        "observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"allOf":[
                            {"contains":{"const":"recording_start"}},
                            {"contains":{"const":"recording_state"}},
                            {"contains":{"const":"recording_stop"}}
                        ]}
                    }}
                }, {
                    "if":{"properties":{"allowed_methods":{"contains":{"enum":["recording_start","recording_state","recording_stop"]}}}},
                    "then":{"required":["allow_recording"],"properties":{"allow_recording":{"const":true}}}
                }, {
                    "if":{"properties":{"allowed_methods":{"contains":{"const":"live_observation_start"}}}},
                    "then":{"properties":{"allowed_methods":{"allOf":[
                        {"contains":{"const":"live_observation_state"}},
                        {"contains":{"const":"live_observation_stop"}}
                    ]}}}
                }, {
                    "if":{"properties":{"allowed_methods":{"contains":{"const":"set_window_frame"}}}},
                    "then":{"required":["observation_mode"],"properties":{
                        "observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"contains":{"const":"get_window_state"}},
                        "allowed_actions":{"contains":native_frame_scope_schema()}
                    }}
                }, {
                    "if":{"properties":{"allowed_actions":{"contains":native_menu_dismiss_scope_schema()}}},
                    "then":{"required":["observation_mode"],"properties":{
                        "observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},
                        "allowed_methods":{"allOf":[{"contains":{"const":"get_window_state"}},{"contains":{"const":"change_window_state"}}]}
                    }}
                }],
                "properties": {
                    "application_label": {"type": "string", "minLength": 1, "maxLength": 80},
                    "target_process_id": {"type": "integer", "minimum": 1},
                    "target_window_handle": {"type": "integer", "minimum": 1},
                    "owned_browser_launch": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["browser", "profile"],
                        "properties": {
                            "browser": {"type": "string", "enum": ["chromium"]},
                            "profile": {"type": "string", "enum": ["isolated_new"]}
                        }
                    },
                    "surface": {"type": "string", "enum": ["window", "browser"]},
                    "observation_mode": {"type": "string", "enum": ["semantic", "pixels_only"], "default": "semantic"},
                    "allow_recording": {"type":"boolean","default":false,
                        "description":"Explicit native pixels video-only permission. Requires recording_start/state/stop and an operator-configured DCC_CUA_RECORDING_OUTPUT_ROOT. The server allocates an immutable task directory; callers cannot configure its root."},
                    "allow_capture_preparation":{"type":"boolean","default":false,
                        "description":"Explicit bounded passive exact-root preparation. Requires pixels_only, get_window_state, begin/state/stop, its closed action, and operator-owned DCC_CUA_CAPTURE_PREPARATION_JOURNAL_ROOT. Does not grant input or caller-selected journal/target; prepared snapshots mint no observation/input token."},
                    "allowed_methods": {
                        "type": "array",
                        "description": "Closed Host methods. Extension calls require an exact task-authorized expected_origin. Click requires semantic/external_effect scope; non-secret type requires semantic/content_change scope. Both require a fresh task-owned snapshot and foreground ready target.",
                        "minItems": 1,
                        "maxItems": MAX_ALLOWED_METHODS,
                        "uniqueItems": true,
                        "items": {"type": "string", "enum": [
                            "get_window_state", "change_window_state", "minimize_window", "set_window_frame", "snapshot", "accessibility_snapshot", "verify_state",
                            "capture_preparation_begin", "capture_preparation_state", "capture_preparation_stop", "capture_preparation_snapshot",
                            "find", "wait_for", "execute_action", "get_session_state",
                            "get_input_state", "session_health", "poll_session_events",
                            "clipboard_capture_secret", "browser_snapshot", "browser_prepare",
                            "browser_navigate", "browser_click", "browser_type", "browser_pointer",
                            "browser_set_input_files", "browser_download", "browser_dialog",
                            "browser_extension_status", "browser_extension_call",
                            "live_observation_start", "live_observation_state", "live_observation_stop",
                            "recording_start", "recording_state", "recording_stop"
                        ]}
                    },
                    "allowed_actions": {
                        "type": "array",
                        "description": "Closed final action scopes; use an empty array for observation-only tasks. pixels_only accepts minimize_window, set_window_frame or the independent dismiss_menu scope with input_kind=window_state, secret_input=false, authorization_category=window_state; these grant no raw input. Grant only pixel actions advertised by this runtime. Windows supports click, double_click, right_click, toggle, drag, keypress, keyboard_shortcut, key_sequence, type and type_chars. macOS and local X11 support foreground click and key_sequence with input_kind=raw_input, secret_input=false, authorization_category=raw_input. execute_action requires an actual supported raw scope. Other modes use click/type action names for browser input methods; only secret-handle browser typing uses browser_type.",
                        "minItems": 0,
                        "maxItems": 32,
                        "uniqueItems": true,
                        "items": action_scope
                    },
                    "allowed_browser_origins": {
                        "type": "array",
                        "maxItems": 32,
                        "uniqueItems": true,
                        "items": {"type": "string", "format": "uri"}
                    },
                    "ttl_minutes": {"type": "integer", "minimum": 1, "maximum": MAX_TTL_MINUTES, "default": DEFAULT_TTL_MINUTES}
                }
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        }),
        json!({
            "name": "task_status",
            "title": "DCC-CUA task status",
            "description": "Read the current state of an exact DCC-CUA task on the same MCP connection that started it. Task IDs are not portable across connections.",
            "inputSchema": task_id_schema(),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "stop_task",
            "title": "Stop DCC-CUA task",
            "description": "Revoke one exact task's runtime lease, then await Host session cleanup, including video and sidecar finalization, and bounded connection completion. Cleanup errors or unknown acknowledgements remain visible; a dropped connection is not a successful stop. Pending Host completion may be checked again with stop_task; it never restarts or replays the task.",
            "inputSchema": task_id_schema(),
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "dcc_cua_metrics",
            "title": "Read DCC-CUA performance metrics",
            "description": "Read connection-scoped debug metrics for request time, protocol message bytes, estimated protocol tokens, explicitly reported model tokens, standalone snapshots, post-action snapshots and post_snapshot reuse. Estimated protocol tokens use ceil(UTF-8 bytes / 4); exact model usage must be reported by the Agent Host.",
            "inputSchema": {"type":"object", "additionalProperties":false, "properties":{}},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "dcc_cua_record_model_usage",
            "title": "Report model token usage",
            "description": "Record exact model input/output token counts supplied by the Agent Host. This only updates local debug counters and grants no task authority. Prefer the one-way notifications/dcc-cua/model_usage message when the client can emit it without a tool round trip.",
            "inputSchema": {
                "type":"object", "additionalProperties":false, "required":["usage"],
                "properties":{"usage":{"type":"object", "additionalProperties":false,
                    "properties":{"input_tokens":{"type":"integer","minimum":0},"output_tokens":{"type":"integer","minimum":0},"prompt_tokens":{"type":"integer","minimum":0},"completion_tokens":{"type":"integer","minimum":0},"cached_input_tokens":{"type":"integer","minimum":0},"model_elapsed_ms":{"type":"integer","minimum":0}},
                    "anyOf":[{"required":["input_tokens"]},{"required":["output_tokens"]},{"required":["prompt_tokens"]},{"required":["completion_tokens"]}]}}
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        }),
        json!({
            "name": "dcc_cua_task_call",
            "title": "Run DCC-CUA task call",
            "description": "Call one closed Host method on the same MCP connection after start_task returned its provider/runtime/PID/HWND binding. execute_action params require observation_id and action {action, input_kind, intent}; semantic input also requires accessibility_state_id and a fresh element reference. Raw x/y are pixels in snapshot.coordinate_space, never UIA virtual-desktop bounds or a resized preview; source_rect already contains native window units, so do not apply DPI again. Keyboard input uses keys, not key. Keep snapshot and action in that same task; observation IDs and opaque references are not reusable after reconnecting or in another CLI invocation. In explicitly granted semantic tasks, supported Windows UIA click, toggle, set_text or set_value may use delivery_mode=background with the latest observation_id, accessibility_state_id and same-task UIA element reference. Take a fresh semantic snapshot and verify the actual post-action state; type, double_click, coordinates and keyboard input do not inherit this UIA background support. A pixels_only snapshot yields a formal observation_id and accessibility_available=false, without semantic element tokens. Its optional capture_diagnostics boolean defaults to false; true adds bounded byte hashes, histograms, timing, and native provenance for the existing two captures without changing pixels or input authority. Semantic snapshots reject this diagnostic opt-in. Its execute_action requires a granted supported raw_input action and that latest observation_id; omit accessibility_state_id, element selectors, secret handles, and input_backend_id. Pixels-only delivery is foreground only, without implicit activation. Pixels-only window state and typed activation refusals expose interaction_recovery: snapshot permission is independent of foreground input readiness. Native background viability hints never authorize pixels-only background input or a task-mode switch to bypass refusal. Do not blindly retry refused activation. In pixels_only tasks, use capture_after for a fresh pixel post-action observation; semantic post snapshots are refused in that mode. minimize_window requires the latest observation and same native instance, consumes it after an attempt, and reports native minimized state. set_window_frame requires a one-use window_state_id from explicit get_window_state no more than five seconds old, and frame={x,y,width,height} in exact i32 physical pixels with checked extents. It moves the same full native instance without activation or z-order change and confirms only an exact bounded native readback; every attempt consumes metadata and prior pixel evidence. Its response state does not mint another token. change_window_state also accepts dismiss_menu only with its independent window_state/dismiss_menu action scope and a one-use window_state_id copied from get_window_state.menu_dismiss_window_state_id, at most five seconds old. Frame and snapshot tokens cannot authorize menu dismissal. It sends one bounded standard native cancellation without activation, keyboard input or application close. Confirmed requires popup_hidden_or_gone, menu_inactive and owner_alive; an unconfirmed attempt is never success and must not be blindly retried. Every attempt consumes metadata and prior observations. Take a fresh observation afterward. Out-of-scope, expired, stopped, changed, or stale targets fail without prompting. Recording requires explicit allow_recording and an immutable task output directory; call recording_start with an empty params object for video-only output. Granted live_observation_start/state/stop expose the same native source. Recording start validates the actual first encoded frame, state reports source pauses and failures, and stop awaits video/sidecar finalization. Never pass credential values; use secret handles in supported semantic/browser tasks.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["task_id", "method", "params"],
                "properties": {
                    "task_id": {"type": "string"},
                    "method": {"type": "string"},
                    "params": {"type": "object"},
                    "include_metrics": {"type":"boolean", "default":false, "description":"When true, add a connection-scoped performance snapshot to result _meta for debugging. It is omitted by default to avoid increasing model message tokens."},
                    "compact_text":{"type":"boolean","default":false,"description":"For accessibility_snapshot, retain the full tree in structuredContent and return a short text block for clients that consume the structured result."}
                },
                "allOf": [{
                    "if":{"required":["compact_text"],"properties":{"compact_text":{"const":true}}},
                    "then":{"properties":{"method":{"const":"accessibility_snapshot"}}}
                }, {
                    "if":{"properties":{"method":{"const":"execute_action"}}},
                    "then":{"properties":{"params": action_params_schema()}}
                }, {
                    "if":{"properties":{"method":{"const":"change_window_state"}}},
                    "then":{"properties":{"params":{"oneOf":[
                        {"type":"object","additionalProperties":false,"required":["operation"],"properties":{"operation":{"enum":["activate","restore_activate"]}}},
                        {"type":"object","additionalProperties":false,"required":["operation","window_state_id"],"properties":{
                            "operation":{"const":"dismiss_menu"},"window_state_id":{"type":"string","pattern":"^native-menu-state-","maxLength":128,
                                "description":"One-use menu_dismiss_window_state_id from get_window_state, at most five seconds old. Frame and snapshot tokens are refused."}
                        }}
                    ]}}}
                }, {
                    "if":{"properties":{"method":{"const":"capture_preparation_begin"}}},
                    "then":{"properties":{"params":{
                        "type":"object","additionalProperties":false,"required":["request"],
                        "properties":{"request":{"type":"object","additionalProperties":false,
                            "required":["window_state_id","lifetime_ms"],"properties":{
                                "window_state_id":{"type":"string","minLength":1,"maxLength":128},
                                "lifetime_ms":{"type":"integer","minimum":1,"maximum":dcc_cua_protocol::capture_preparation::MAX_PREPARATION_LIFETIME_MS}
                            }}}
                    }}}
                }, {
                    "if":{"properties":{"method":{"enum":["capture_preparation_state","capture_preparation_stop","capture_preparation_snapshot"]}}},
                    "then":{"properties":{"params":{"type":"object","maxProperties":0}}}
                }, {
                    "if":{"properties":{"method":{"const":"set_window_frame"}}},
                    "then":{"properties":{"params":{
                        "type":"object","additionalProperties":false,"required":["window_state_id","frame"],
                        "properties":{
                            "window_state_id":{"type":"string","minLength":1,"maxLength":128,
                                "description":"One-use token from explicit pixels_only get_window_state, at most five seconds old. Snapshot observation IDs do not authorize this method."},
                            "frame":{"type":"object","additionalProperties":false,"required":["x","y","width","height"],
                                "properties":{
                                    "x":{"type":"integer","minimum":-2147483648,"maximum":2147483647},
                                    "y":{"type":"integer","minimum":-2147483648,"maximum":2147483647},
                                    "width":{"type":"integer","minimum":1,"maximum":2147483647},
                                    "height":{"type":"integer","minimum":1,"maximum":2147483647}
                                }
                            }
                        }
                    }}}
                }, {
                    "if": {"properties": {"method": {"const": "snapshot"}}},
                    "then": {"properties": {"params": {
                        "properties": {"capture_diagnostics": {
                            "type": "boolean", "default": false,
                            "description": "Opt-in content-free byte diagnostics on an explicit pixels_only snapshot only. No extra capture or input permission."
                        }}
                    }}}
                }, {
                    "if":{"properties":{"method":{"const":"recording_start"}}},
                    "then":{"properties":{"params":{"type":"object","additionalProperties":false,
                        "properties":{"request":{"type":"object","additionalProperties":false,
                            "properties":{"recording_audio":{"enum":["none","target_process_tree"]},"record_video":{"const":true},"output_dir":{"type":"string",
                                "description":"Optional exact task-authorized directory. The server supplies its immutable path and rejects a different value."}}}}
                    }}}
                }, {
                    "if":{"properties":{"method":{"const":"live_observation_start"}}},
                    "then":{"properties":{"params":{"type":"object","additionalProperties":false,
                        "properties":{"request":{"type":"object","additionalProperties":false,
                            "properties":{"fps":{"type":"integer","minimum":1,"maximum":30},
                                "max_dimension":{"type":"integer","minimum":256,"maximum":4096}}}}
                    }}}
                }, {
                    "if":{"properties":{"method":{"enum":["recording_state","recording_stop","live_observation_state","live_observation_stop"]}}},
                    "then":{"properties":{"params":{"type":"object","maxProperties":0}}}
                }]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        }),
    ];
    recording_audio::configure_schema(&mut tools[0]);
    tools[0]["inputSchema"]["properties"]["allow_native_capture_popup_keyboard"] =
        capture_popup::owner_schema();
    workflows::extend_tool_schema(&mut tools);
    tools
}

fn task_id_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["task_id"],
        "properties": {"task_id": {"type": "string"}}
    })
}

fn action_params_schema() -> Value {
    json!({
        "type":"object", "required":["observation_id","action"],
        "properties":{
            "observation_id":{"type":"string","minLength":1,"description":"Copy the latest same-task observation_id; never reuse it after an action attempt."},
            "accessibility_state_id":{"type":"string","description":"Required for semantic input: copy the latest same-task accessibility_state_id."},
            "capture_after":{"type":"boolean","default":false,"description":"When true, the response contains the fresh post_snapshot for the next action. Reuse that observation_id directly and do not call snapshot again unless observation_required is true."},
            "post_snapshot_delay_ms":{"type":"integer","minimum":0},
            "post_snapshot_mode":{"type":"string","enum":["full","semantic"]},
            "action":{
                "type":"object", "required":["action","input_kind","intent"],
                "properties":{
                    "action":{"type":"string","enum":TrustedTaskActionScope::NATIVE_ACTIONS},
                    "input_kind":{"type":"string","enum":["semantic","raw_input"]},
                    "intent":{"type":"string","description":"Required audit context. For bounded raw navigation use navigate or ordinary_edit. Semantic authorization uses the observed policy_tier, not this label."},
                    "delivery_mode":{"type":"string","enum":["foreground","background"]},
                    "element_index":{"type":"integer","minimum":0},
                    "element_token":{"type":"string","minLength":1,"description":"Opaque reference from the latest same-task UIA observation; do not convert its virtual-desktop bounds to guessed coordinates."},
                    "x":{"type":"number","minimum":0,"description":"X in snapshot.coordinate_space image pixels, not UIA/desktop bounds or a resized preview. source_rect already contains native window units; do not multiply DPI again."},
                    "y":{"type":"number","minimum":0,"description":"Y in the latest delivered image, with the same coordinate contract as x."},
                    "button":{"type":"string","enum":["left","middle","right"]},
                    "path":{"type":"array","minItems":2,"maxItems":201,"items":{"type":"object","additionalProperties":false,"required":["x","y"],"properties":{"x":{"type":"number","minimum":0},"y":{"type":"number","minimum":0}}},"description":"A bounded pixel trajectory in the latest screenshot. Use multiple points for one continuous pen or brush stroke. A two-point path may use steps for linear interpolation."},
                    "duration_ms":{"type":"integer","minimum":1,"maximum":10000,"description":"Total duration for a drag or held keypress. Omit it to use the route default."},
                    "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Interpolation segments for a two-point drag. For a sampled multi-point brush path, send the points directly and omit steps."},
                    "keys":{"type":"array","items":{"type":"string"},"description":"Keyboard input uses keys, not key. keypress holds one chord. Foreground key_sequence in supported Windows, macOS and local X11 pixels_only tasks taps and releases each entry in order (e.g. [F2, CTRL+A, A, A, ENTER]). Grant key_sequence explicitly. Use one initial observation and optional final capture."},
                    "delay_ms":{"type":"integer","minimum":0,"maximum":200,"description":"For key_sequence, pause between taps in milliseconds (default 40)."},
                    "modifiers":{"type":"array","items":{"type":"string"},"description":"Keyboard modifiers for supported raw-input actions."},
                    "text":{"type":"string","description":"Non-secret text only. Credentials must use a trusted secret_handle."}
                },
                "allOf":[{
                    "if":{"properties":{"action":{"const":"key_sequence"}}},
                    "then":{"required":["keys"],"properties":{
                        "input_kind":{"const":"raw_input"},"delivery_mode":{"const":"foreground"},
                        "keys":{"minItems":1,"maxItems":64,"items":{"minLength":1,"maxLength":64}}
                    }}
                }]
            }
        },
        "allOf":[{
            "if":{"properties":{"action":{"properties":{"input_kind":{"const":"semantic"}}}}},
            "then":{"required":["accessibility_state_id"]}
        }]
    })
}
