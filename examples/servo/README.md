# SchnellUI + Servo example

This example embeds a Servo 0.4 webview in SchnellUI. Servo renders the page to
an offscreen RGBA frame; SchnellUI displays that frame as an image, supplies the
native toolbar, and forwards focused pointer, keyboard, wheel, focus, and IME
events back to Servo.

Run the deterministic built-in page interactively:

```bash
cargo run -p servo_demo --release -- --windowed
```

Capture it headlessly, or open another URL:

```bash
cargo run -p servo_demo --release -- --out servo.png
cargo run -p servo_demo --release -- --url https://servo.org --windowed
```

Session persistence is opt-in:

```bash
cargo run -p servo_demo --release -- --windowed --state target/servo-session.json
```

The persisted state includes the active tab, navigation history, zoom, scroll
position, and cookies. Close the native window with Escape or its window button.
