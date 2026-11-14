This host is adapted from knave-de/knave-ui's `knave-app` at
77bb28aab7bb07eae0a01a1611b6c89c2f51ad07. Its MIT license is retained here.

Notes needs document editing independently of the toolkit's form text inputs.
The local host adds native-key, committed-text, and pointer interception;
clipboard effects; custom display-list painting; custom IME state; resize and
close-request hooks; a minimum window size; and the knave-notepad Wayland app ID.
UI widgets, text measurement, display commands, rendering, and clipboard/IME
hosting continue to use the Knave UI implementation.

For visual QA, debug builds support `KNAVE_NOTEPAD_CAPTURE_REQUEST=/path/request`.
Write an absolute output PNG path into that request file and cause a redraw
(e.g. move the pointer). The host reads back the actual rendered swapchain frame,
writes the PNG, and removes the request file. No capture runs unless explicitly
configured. This is useful when Xvfb cannot capture Vulkan presentation through
XGetImage. Release builds do not enable capture.
