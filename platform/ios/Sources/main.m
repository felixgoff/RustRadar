// The app's entry point (doc 07 §13.5). Everything else is Rust: rustradar_ios_main, in
// librustradar_app.a (crates/app/src/lib.rs), runs winit's event loop, which starts UIKit
// (UIApplicationMain) on this, the main thread, and never returns.

extern void rustradar_ios_main(void);

int main(int argc, char *argv[]) {
    (void)argc;
    (void)argv;
    rustradar_ios_main();
    return 0;
}
