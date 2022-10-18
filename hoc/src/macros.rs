macro_rules! args_summary {
    {
        $($field:ident {
            $($args:ident = $values:literal),* $(,)?
        })+
    } => {
        args_summary! {
            @impl ($($field {
                $($args = $values,)*
            })+) => {
                default_output: {}
                help_output: {}
                long_help_output: {}
            }
        }
    };

    {
        @impl ($field:ident {
            help = $help:literal,
        } $($fields:tt)*) => {
            default_output: {
                $($default_output:tt)*
            }
            help_output: {
                $($help_output:tt)*
            }
            long_help_output: {
                $($long_help_output:tt)*
            }
        }
    } => {
        args_summary! {
            @impl ($($fields)*) => {
                default_output: {
                    $($default_output)*
                }
                help_output: {
                    $($help_output)*
                    pub fn $field() -> &'static str {
                        $help
                    }
                }
                long_help_output: {
                    $($long_help_output)*
                }
            }
        }
    };

    {
        @impl ($field:ident {
            default = $default:literal,
            help = $help:literal,
        } $($fields:tt)*) => {
            default_output: {
                $($default_output:tt)*
            }
            help_output: {
                $($help_output:tt)*
            }
            long_help_output: {
                $($long_help_output:tt)*
            }
        }
    } => {
        args_summary! {
            @impl ($($fields)*) => {
                default_output: {
                    $($default_output)*
                    pub fn $field() -> &'static str {
                        $default
                    }
                }
                help_output: {
                    $($help_output)*
                    pub fn $field() -> &'static str {
                        concat!($help, " [default: ", $default, "]")
                    }
                }
                long_help_output: {
                    $($long_help_output)*
                }
            }
        }
    };

    {
        @impl ($field:ident {
            help = $help:literal,
            long_help = $long_help:literal,
        } $($fields:tt)*) => {
            default_output: {
                $($default_output:tt)*
            }
            help_output: {
                $($help_output:tt)*
            }
            long_help_output: {
                $($long_help_output:tt)*
            }
        }
    } => {
        args_summary! {
            @impl ($field {
                help = $help,
            } $($fields)*) => {
                default_output: {
                    $($default_output)*
                }
                help_output: {
                    $($help_output)*
                }
                long_help_output: {
                    $($long_help_output)*
                    pub fn $field() -> &'static str {
                        concat!($help, "\n\n", $long_help)
                    }
                }
            }
        }
    };

    {
        @impl ($field:ident {
            default = $default:literal,
            help = $help:literal,
            long_help = $long_help:literal,
        } $($fields:tt)*) => {
            default_output: {
                $($default_output:tt)*
            }
            help_output: {
                $($help_output:tt)*
            }
            long_help_output: {
                $($long_help_output:tt)*
            }
        }
    } => {
        args_summary! {
            @impl ($field {
                default = $default,
                help = $help,
            } $($fields)*) => {
                default_output: {
                    $($default_output)*
                }
                help_output: {
                    $($help_output)*
                }
                long_help_output: {
                    $($long_help_output)*
                    pub fn $field() -> &'static str {
                        concat!($help, "\n\n", $long_help, "\n\n[default: ", $default, "]")
                    }
                }
            }
        }
    };

    {
        @impl () => {
            default_output: {
                $($default_output:tt)*
            }
            help_output: {
                $($help_output:tt)*
            }
            long_help_output: {
                $($long_help_output:tt)*
            }
        }
    } => {
        mod default {
            $($default_output)*
        }

        mod help {
            $($help_output)*

            pub mod long {
                $($long_help_output)*
            }
        }
    };
}

macro_rules! prompt_arg {
    ($self:ident, $field:ident $(,)?) => {
        $crate::prompt::Prompt::get($self.$field, stringify!($field))
    };
}

macro_rules! prompt_arg_default {
    ($self:ident, $field:ident $(,)?) => {
        $crate::prompt::Prompt::get_or($self.$field, stringify!($field), default::$field())
    };
}

macro_rules! progress {
    ($($args:tt)*) => {
        $crate::logger::render::RENDER_THREAD
            .get()
            .expect(EXPECT_RENDER_THREAD_INITIALIZED)
            .push_progress(format!($($args)*))
    };
}

macro_rules! select {
    ($($args:tt)*) => {
        $crate::prompt::select(format!($($args)*))
    };
}

macro_rules! put {
    ($value:expr => $($args:tt)*) => {
        $crate::context::CONTEXT
            .get()
            .expect(EXPECT_CONTEXT_INITIALIZED)
            .kv_put_value(
                {
                    let __args = format_args!($($args)*);
                    if let Some(__args) = __args.as_str() {
                        ::std::borrow::Cow::Borrowed(__args)
                    } else {
                        ::std::borrow::Cow::Owned(__args.to_string())
                    }
                },
                $value,
            )
    };
}
