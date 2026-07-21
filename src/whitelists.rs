//! Names used implicitly by common Python framework protocols.
//!
//! Vulture ships these as small Python modules and loads one when an import with
//! the same name is seen. Keeping the data native avoids parsing synthetic files
//! while preserving the same project-wide name semantics.

pub(crate) struct Whitelist {
    pub(crate) lexical: &'static [&'static str],
    pub(crate) members: &'static [&'static str],
}

const EMPTY: &[&str] = &[];

pub(crate) fn for_import(name: &str) -> Option<Whitelist> {
    let whitelist = match name {
        "argparse" => Whitelist {
            lexical: &["argparse"],
            members: &["epilog", "_fill_text", "_get_help_string"],
        },
        "ast" => Whitelist {
            lexical: EMPTY,
            members: &[
                "visit_Assert",
                "visit_Assign",
                "visit_AsyncFor",
                "visit_AsyncFunctionDef",
                "visit_AsyncWith",
                "visit_Attribute",
                "visit_AugAssign",
                "visit_Await",
                "visit_BinOp",
                "visit_BoolOp",
                "visit_Bytes",
                "visit_Call",
                "visit_ClassDef",
                "visit_Compare",
                "visit_Constant",
                "visit_Delete",
                "visit_Dict",
                "visit_DictComp",
                "visit_ExceptHandler",
                "visit_Exec",
                "visit_Expr",
                "visit_Expression",
                "visit_ExtSlice",
                "visit_For",
                "visit_FunctionDef",
                "visit_GeneratorExp",
                "visit_Global",
                "visit_If",
                "visit_IfExp",
                "visit_Import",
                "visit_ImportFrom",
                "visit_Index",
                "visit_Interactive",
                "visit_Lambda",
                "visit_List",
                "visit_ListComp",
                "visit_MatchClass",
                "visit_Module",
                "visit_Name",
                "visit_NameConstant",
                "visit_Nonlocal",
                "visit_Num",
                "visit_Print",
                "visit_Raise",
                "visit_Repr",
                "visit_Return",
                "visit_Set",
                "visit_SetComp",
                "visit_Slice",
                "visit_Starred",
                "visit_Str",
                "visit_Subscript",
                "visit_Suite",
                "visit_Try",
                "visit_TryExcept",
                "visit_TryFinally",
                "visit_Tuple",
                "visit_UnaryOp",
                "visit_While",
                "visit_With",
                "visit_Yield",
                "visit_YieldFrom",
                "visit_alias",
                "visit_arg",
                "visit_arguments",
                "visit_comprehension",
                "visit_keyword",
            ],
        },
        "collections" => Whitelist {
            lexical: &["collections"],
            members: &["default_factory"],
        },
        "ctypes" => Whitelist {
            lexical: &["_CFuncPtr", "_Pointer"],
            members: &["argtypes", "errcheck", "restype", "contents"],
        },
        "enum" => Whitelist {
            lexical: &["enum"],
            members: &["_name_", "_value_"],
        },
        "logging" => Whitelist {
            lexical: &["logging"],
            members: &["filter", "propagate", "emit"],
        },
        "pint" => Whitelist {
            lexical: &["pint"],
            members: &["default_format"],
        },
        "socketserver" => Whitelist {
            lexical: &["socketserver"],
            members: &["allow_reuse_address"],
        },
        "ssl" => Whitelist {
            lexical: &["ssl"],
            members: &[
                "check_hostname",
                "hostname_checks_common_name",
                "keylog_filename",
                "maximum_version",
                "minimum_version",
                "options",
                "post_handshake_auth",
                "sni_callback",
                "sslobject_class",
                "sslsocket_class",
                "verify_flags",
                "verify_mode",
            ],
        },
        "string" => Whitelist {
            lexical: &["string"],
            members: &[
                "check_unused_args",
                "convert_field",
                "format",
                "format_field",
                "get_field",
                "get_value",
                "parse",
                "vformat",
            ],
        },
        "sys" => Whitelist {
            lexical: &["sys"],
            members: &["excepthook", "stderr", "stdin", "stdout"],
        },
        "threading" => Whitelist {
            lexical: &["threading"],
            members: &["daemon", "name", "run"],
        },
        "unittest" => Whitelist {
            lexical: &["TestCase", "mock"],
            members: &[
                "setUp",
                "tearDown",
                "setUpClass",
                "tearDownClass",
                "run",
                "skipTest",
                "debug",
                "failureException",
                "longMessage",
                "maxDiff",
                "subTest",
                "return_value",
                "side_effect",
            ],
        },
        _ => return None,
    };
    Some(whitelist)
}

#[cfg(test)]
mod tests {
    use super::for_import;

    #[test]
    fn recognizes_vulture_packaged_whitelists() {
        assert!(for_import("ast").is_some());
        assert!(for_import("unittest").is_some());
        assert!(for_import("unknown").is_none());
    }
}
