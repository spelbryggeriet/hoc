macro_rules! cmd_diagnostics {
    ($type:ty) => {{
        debug!("Running {} command", <$type>::command().get_name());
    }};
}

macro_rules! arg_diagnostics {
    ($command:ident.$field:ident) => {{
        if let Some(__inner) = &$command.$field {
            arg_diagnostics!($field, __inner);
        }
    }};

    ($field:ident) => {{
        if let Some(__inner) = &$field {
            arg_diagnostics!($field, __inner);
        }
    }};

    ($field:ident, $value:expr) => {{
        let message = ::heck::ToTitleCase::to_title_case(stringify!($field));
        debug!("{message}: {}", $value);
    }};
}

macro_rules! get_arg {
    ($self:ident.$field:ident $(, default = $command:ident)? $(,)?) => {{

        let value = if let Some(inner) = $self.$field {
            arg_diagnostics!($field, inner);
            inner
        } else {
            let message = ::heck::ToTitleCase::to_title_case(stringify!($field));
            let value = prompt!("{message}")
                $(.with_default(default::$command::$field()))?
                .get()?;
            value
        };

        ::anyhow::Ok(value)
    }};
}

macro_rules! get_secret_arg {
    ($self:ident.$field:ident $(,)?) => {{
        let value = if let Some(inner) = $self.$field {
            arg_diagnostics!($field, inner);
            inner
        } else {
            let message = ::heck::ToTitleCase::to_title_case(stringify!($field));
            let value = prompt!("{message}").secret().get()?;
            value
        };

        ::anyhow::Ok(value)
    }};
}

macro_rules! commands_summary {
    {
        $( $command:ident { $($inner_tokens:tt)* } )*
    } => {
        commands_summary! {
            @impl ($( $command { $($inner_tokens)* } )*) => {
                default_output {}
                help_output {}
                long_help_output {}
                default_mod_output {}
                help_mod_output {}
                long_help_mod_output {}
            }
        }
    };

    {
        @impl
            ($command:ident {
                $field:ident {
                    help = $help:literal $(,)?
                } $($fields:tt)*
            } $($commands:tt)*) => {
                default_output { $($default_output:tt)* }
                help_output { $($help_output:tt)* }
                long_help_output { $($long_help_output:tt)* }
                $($rest_output:tt)*
            }
    } => {
        commands_summary! {
            @impl ($command { $($fields)* } $($commands)*) => {
                default_output { $($default_output)* }
                help_output {
                    $($help_output)*
                    pub fn $field() -> &'static str {
                        $help
                    }
                }
                long_help_output { $($long_help_output)* }
                $($rest_output)*
            }
        }
    };

    {
        @impl
            ($command:ident {
                $field:ident {
                    default = $default:literal,
                    help = $help:literal $(,)?
                } $($fields:tt)*
            } $($commands:tt)*) => {
                default_output { $($default_output:tt)* }
                help_output { $($help_output:tt)* }
                long_help_output { $($long_help_output:tt)* }
                $($rest_output:tt)*
            }
    } => {
        commands_summary! {
            @impl ($command { $($fields)* } $($commands)*) => {
                default_output {
                    $($default_output)*
                    pub fn $field() -> &'static str {
                        $default
                    }
                }
                help_output {
                    $($help_output)*
                    pub fn $field() -> &'static str {
                        concat!($help, " [default: ", $default, "]")
                    }
                }
                long_help_output { $($long_help_output)* }
                $($rest_output)*
            }
        }
    };

    {
        @impl
            ($command:ident {
                $field:ident {
                    help = $help:literal,
                    long_help = $long_help:literal $(,)?
                } $($fields:tt)*
            } $($commands:tt)*) => {
                default_output { $($default_output:tt)* }
                help_output { $($help_output:tt)* }
                long_help_output { $($long_help_output:tt)* }
                $($rest_output:tt)*
            }
    } => {
        commands_summary! {
            @impl ($command {
                $field {
                    help = $help,
                } $($fields)*
            } $($commands)*) => {
                default_output { $($default_output)* }
                help_output { $($help_output)* }
                long_help_output {
                    $($long_help_output)*
                    pub fn $field() -> &'static str {
                        concat!($help, "\n\n", $long_help)
                    }
                }
                $($rest_output)*
            }
        }
    };

    {
        @impl ($command:ident {
            $field:ident {
                default = $default:literal,
                help = $help:literal,
                long_help = $long_help:literal $(,)?
            } $($fields:tt)*
        } $($commands:tt)*) => {
            default_output { $($default_output:tt)* }
            help_output { $($help_output:tt)* }
            long_help_output { $($long_help_output:tt)* }
            $($rest_output:tt)*
        }
    } => {
        commands_summary! {
            @impl ($command {
                $field {
                    default = $default,
                    help = $help,
                } $($fields)*
            } $($commands)*) => {
                default_output { $($default_output)* }
                help_output { $($help_output)* }
                long_help_output {
                    $($long_help_output)*
                    pub fn $field() -> &'static str {
                        concat!($help, "\n\n", $long_help, "\n\n[default: ", $default, "]")
                    }
                }
                $($rest_output)*
            }
        }
    };


    {
        @impl ($command:ident {} $($commands:tt)*) => {
            default_output { $($default_output:tt)* }
            help_output { $($help_output:tt)* }
            long_help_output { $($long_help_output:tt)* }
            default_mod_output { $($default_mod_output:tt)* }
            help_mod_output { $($help_mod_output:tt)* }
            long_help_mod_output { $($long_help_mod_output:tt)* }
        }
    } => {
        commands_summary! {
            @impl ($($commands)*) => {
                default_output {}
                help_output {}
                long_help_output {}
                default_mod_output {
                    $($default_mod_output)*
                    pub mod $command {
                        $($default_output)*
                    }
                }
                help_mod_output {
                    $($help_mod_output)*
                    pub mod $command {
                        $($help_output)*
                    }
                }
                long_help_mod_output {
                    $($long_help_mod_output)*
                    pub mod $command {
                        $($long_help_output)*
                    }
                }
            }
        }
    };

    {
        @impl () => {
            default_output {}
            help_output {}
            long_help_output {}
            default_mod_output { $($default_mod_output:tt)* }
            help_mod_output { $($help_mod_output:tt)* }
            long_help_mod_output { $($long_help_mod_output:tt)* }
        }
    } => {
        mod default { $($default_mod_output)* }

        mod help { $($help_mod_output)* }

        mod long_help { $($long_help_mod_output)* }
    };
}
