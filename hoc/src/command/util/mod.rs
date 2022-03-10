use hoclib::cmd_macros;

cmd_macros!(
    adduser,
    cat,
    chmod,
    chpasswd,
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

pub mod cidr;
pub mod disk;
pub mod os;
