#[derive(Copy, Clone)]
pub struct PrefixPrefs<'a> {
    pub connector: &'a str,
    pub flag: &'a str,
    pub label: &'a str,
}

impl<'a> PrefixPrefs<'a> {
    pub fn with_connector(connector: &'a str) -> Self {
        Self {
            connector,
            flag: " ",
            label: "",
        }
    }

    pub fn in_status() -> Self {
        Self::with_connector("╟╴")
    }

    pub fn in_status_overflow() -> Self {
        Self::with_connector("║ ")
    }

    pub fn flag(mut self, flag: &'a str) -> Self {
        self.flag = flag;
        self
    }

    pub fn label(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }
}
