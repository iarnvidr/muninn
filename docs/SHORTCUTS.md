# Keyboard shortcuts and conflicts

This table describes the web UI handlers, not operating-system/browser guarantees. Ctrl means Control; on macOS the window shortcuts accept Command too.

| Scope / keys | Implemented action | Conditions / conflict notes |
|---|---|---|
| Ctrl/⌘+T, W, L, R | New tab, close tab, focus address bar, reload | Window handler (`ui/lib.js` shortcutAction). Alt combinations ignored. Ctrl+[ and Ctrl+] are not handled; Meta+[ and Meta+] navigate back/forward. Shift is accepted only for bracket keys, so Ctrl+Shift+Tab is **not** a tab-cycle shortcut. |
| Space or K | Toggle playback | Player handler only. |
| J/L; Left/Right arrows | Seek -10/+10 seconds; arrows -5/+5 seconds | Player handler only. |
| Up/Down arrows | Volume +5/-5 | Player handler only. |
| F, M, I | Fullscreen, mute, picture-in-picture | Browser/platform capability may constrain fullscreen/PiP. |
| Comma/period; < / > | Frame backward/forward; speed down/up | Availability depends on active video/playback support. |
| Home/End; 0–9 | Start/end; seek to 0–90% | Player handler only. |
| Shift+N, Shift+P | Upcoming queue navigation | Separate upcoming-queue actions; not implemented as chapter navigation. |

Player actions are suppressed while composing IME text, with Ctrl/Meta/Alt held, or in typing/editable targets and native BUTTON, SELECT, TEXTAREA, or INPUT controls (including range inputs). Captions and chapter shortcuts are not implemented in the player key-action mapping. Theatre mode exists as an on-screen control (`ui/watch.js`), but has no keyboard action in that mapping. Browser/native control behavior is outside this table. See `ui/lib.js` (`shortcutAction`, `playerKeyAction`) and unit coverage in `ui/test/lib.test.js`.
