macro_rules! diagnostics {
    ($type:ty) => {{
        debug!("Running {} action", <$type>::command().get_name());
    }};
}

macro_rules! get_arg {
    ($self:ident.$field:ident $(, default = $action:ident)? $(,)?) => {
        async {
            let __message = ::heck::ToTitleCase::to_title_case(stringify!($field));
            if let Some(__inner) = $self.$field {
                info!("{__message}: {__inner}");
                Result::<_, ::anyhow::Error>::Ok(__inner)
            } else {
                let __value = prompt!("{__message}")
                    $(.with_default(default::$action::$field()))?
                    .await?;
                info!("{__message}: {__value}");
                Result::<_, ::anyhow::Error>::Ok(__value)
            }
        }
    };
}

macro_rules! get_secret_arg {
    ($self:ident.$field:ident $(,)?) => {
        async {
            let __message = ::heck::ToTitleCase::to_title_case(stringify!($field));
            if let Some(__inner) = $self.$field {
                info!("{__message}: {__inner}");
                Result::<_, ::anyhow::Error>::Ok(__inner)
            } else {
                let __value = prompt!("{__message}").as_secret().await?;
                info!("{__message}: {__value}");
                Result::<_, ::anyhow::Error>::Ok(__value)
            }
        }
    };
}

macro_rules! actions_summary {
    {
        $( $action:ident { $($inner_tokens:tt)* } )*
    } => {
        actions_summary! {
            @impl ($( $action { $($inner_tokens)* } )*) => {
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
            ($action:ident {
                $field:ident {
                    help = $help:literal $(,)?
                } $($fields:tt)*
            } $($actions:tt)*) => {
                default_output { $($default_output:tt)* }
                help_output { $($help_output:tt)* }
                long_help_output { $($long_help_output:tt)* }
                $($rest_output:tt)*
            }
    } => {
        actions_summary! {
            @impl ($action { $($fields)* } $($actions)*) => {
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
            ($action:ident {
                $field:ident {
                    default = $default:literal,
                    help = $help:literal $(,)?
                } $($fields:tt)*
            } $($actions:tt)*) => {
                default_output { $($default_output:tt)* }
                help_output { $($help_output:tt)* }
                long_help_output { $($long_help_output:tt)* }
                $($rest_output:tt)*
            }
    } => {
        actions_summary! {
            @impl ($action { $($fields)* } $($actions)*) => {
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
            ($action:ident {
                $field:ident {
                    help = $help:literal,
                    long_help = $long_help:literal $(,)?
                } $($fields:tt)*
            } $($actions:tt)*) => {
                default_output { $($default_output:tt)* }
                help_output { $($help_output:tt)* }
                long_help_output { $($long_help_output:tt)* }
                $($rest_output:tt)*
            }
    } => {
        actions_summary! {
            @impl ($action {
                $field {
                    help = $help,
                } $($fields)*
            } $($actions)*) => {
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
        @impl ($action:ident {
            $field:ident {
                default = $default:literal,
                help = $help:literal,
                long_help = $long_help:literal $(,)?
            } $($fields:tt)*
        } $($actions:tt)*) => {
            default_output { $($default_output:tt)* }
            help_output { $($help_output:tt)* }
            long_help_output { $($long_help_output:tt)* }
            $($rest_output:tt)*
        }
    } => {
        actions_summary! {
            @impl ($action {
                $field {
                    default = $default,
                    help = $help,
                } $($fields)*
            } $($actions)*) => {
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
        @impl ($action:ident {} $($actions:tt)*) => {
            default_output { $($default_output:tt)* }
            help_output { $($help_output:tt)* }
            long_help_output { $($long_help_output:tt)* }
            default_mod_output { $($default_mod_output:tt)* }
            help_mod_output { $($help_mod_output:tt)* }
            long_help_mod_output { $($long_help_mod_output:tt)* }
        }
    } => {
        actions_summary! {
            @impl ($($actions)*) => {
                default_output {}
                help_output {}
                long_help_output {}
                default_mod_output {
                    $($default_mod_output)*
                    pub mod $action {
                        $($default_output)*
                    }
                }
                help_mod_output {
                    $($help_mod_output)*
                    pub mod $action {
                        $($help_output)*
                    }
                }
                long_help_mod_output {
                    $($long_help_mod_output)*
                    pub mod $action {
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
