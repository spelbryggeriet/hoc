use hoclib::cmd_macros;

cmd_macros!(
    adduser,
    arp,
    cat,
    chmod,
    cmd_file => "file",
    dd,
    deluser,
    diskutil,
    hdiutil,
    mkdir,
    pkill,
    rm,
    sed,
    sshd,
    sync,
    systemctl,
    tee,
    test,
    usermod,
);

pub mod disk;
pub mod image;
