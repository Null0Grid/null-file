fn main() {
    // The Wayland app_id (and X11 WM_CLASS) come from GTK's program name. Set it
    // before GTK starts so KWin and GNOME match the window to
    // dev.nullgrid.file.desktop: right icon in the title bar and taskbar.
    #[cfg(target_os = "linux")]
    glib::set_prgname(Some("dev.nullgrid.file"));
    null_file_lib::run()
}
