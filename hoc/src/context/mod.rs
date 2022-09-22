use self::kv::Kv;

mod kv;

pub struct Context {
    pub kv: Kv,
}

impl Context {
    pub fn new() -> Self {
        Self {
            kv: Kv::new("~/.config/hoc/context.yaml"),
        }
    }
}
