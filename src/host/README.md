This host is adapted from knave-de/knave-ui's `knave-app` at
6d32cccde0ee1b07b8a876c5439d595a7f6a89ee. Its MIT license is retained here.

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

Desktop theme startup detection and live Settings portal subscriptions follow
upstream knave-app. Updates invoke `theme_policy_changed` and `refresh_theme`,
then redraw; custom painting resolves scene colors on each frame.
