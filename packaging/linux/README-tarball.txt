Null File @VERSION@ for Linux (@ARCH@)
=========================================

Run it straight from this folder:

    ./null-file

Or install it (adds it to your app menu so it can open folders):

    ./install.sh                 just for you, into ~/.local (no sudo)
    sudo ./install.sh --system   for everyone, into /usr/local
    ./install.sh --default       also make it the default file manager

To remove it: ./uninstall.sh (or sudo ./uninstall.sh --system).

Needs: glibc 2.35 or newer, and WebKitGTK 4.1 (most desktops already have it):
    Ubuntu/Debian:   sudo apt install libwebkit2gtk-4.1-0
    Fedora:          sudo dnf install webkit2gtk4.1
    openSUSE:        sudo zypper install libwebkit2gtk-4_1-0
    Arch:            sudo pacman -S webkit2gtk-4.1
(RHEL, Rocky and Alma don't have it: use the Flatpak there.)

More: https://github.com/Null0Grid/null-file
