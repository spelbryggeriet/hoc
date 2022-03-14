use hoclib::cmd_macros;

cmd_macros!(
    adduser,
    apt_key => "apt-key",
    cat,
    chmod,
    chpasswd,
    cmd_file => "file",
    curl,
    dd,
    df,
    deluser,
    diskutil,
    lsb_release,
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
