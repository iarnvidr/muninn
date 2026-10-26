# Keyboard shortcuts and conflicts

This table describes the web UI handlers, not operating-system/browser guarantees. Ctrl means Control; on macOS the window shortcuts accept Command too.

| Scope / keys | Implemented action | Conditions / conflict notes |
|---|---|---|
| Ctrl/⌘+T, W, R | New tab, close tab, reload | Window handler (`ui/lib.js` shortcutAction). Alt combinations ignored. Ctrl+[ and Ctrl+] are not handled; Meta+[ and Meta+] navigate back/forward. These are window controls, separate from tab-strip focus cycling. |
| Ctrl/⌘+L or K | Open and focus universal search/address input | Does not create a tab; macOS native menu accelerators also work with the YouTube webview focused. Enter submits a YouTube URL directly or fetches online search results. |
| Search: arrows or h/j/k/l | Move through results and between types | Down enters results from the query. hjkl applies only outside text input. Up/Down crosses online/local bands; Left/Right changes type. Enter opens a result; Escape returns to the query. |
| Space or K | Toggle playback | Player handler only. |
| J/L; Left/Right arrows | Seek -10/+10 seconds; arrows -5/+5 seconds | Player handler only. |
| Up/Down arrows | Volume +5/-5 | Player handler only. |
| F, M, I, T | Fullscreen, mute, picture-in-picture, theatre mode | T toggles this player's theatre layout. Native YouTube controls own their native theatre shortcut in the embedded/native view; Muninn does not override it with a window command. Browser/platform capability may constrain fullscreen/PiP. |
| Comma/period; < / > | Step backward/forward by approximately 1/30 second; speed down/up | Frame-step seeks are approximate time offsets, not frame-accurate stepping; only when paused. Availability depends on active video/playback support. |
| Home/End; 0–9 | Start/end; seek to 0–90% | Player handler only. |
| Ctrl+Tab, Ctrl+Shift+Tab | Focus and activate next/previous tab | Tab-strip handler only, when focus is on the tab strip; wraps around. This is tab-strip focus cycling, not a window-control shortcut. Plain Tab is not intercepted. |
| Shift+N, Shift+P | Upcoming queue navigation | Implemented when an upcoming queue exists; these navigate queue items, not chapters. |

Tab-strip cycling is handled separately by `ui/app.js`: Ctrl+Tab and Ctrl+Shift+Tab cycle forward/backward only when focus is on the tab strip. Plain arrows, Home/End, Delete, and Backspace are never tab-strip actions; their meaning depends on player, native YouTube page, or focused control. Player actions are suppressed while composing IME text, with Ctrl/Meta/Alt held, or in typing/editable targets and native BUTTON, SELECT, TEXTAREA, or INPUT controls (including range inputs). Captions and chapter shortcuts are not implemented in the player key-action mapping. Theatre mode is also available from the player with T; the native YouTube view retains its own theatre shortcut and is not overridden by a window-level command. See `ui/lib.js` (`shortcutAction`, `playerKeyAction`), `ui/app.js` (tab-strip handler), and unit coverage in `ui/test/lib.test.js`.
