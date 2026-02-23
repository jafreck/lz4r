// cli module — Rust port of lz4-1.10.0/programs/lz4cli.c
//
// Submodules are added task by task:
//   task-029: constants
//   task-030: help
//   task-031: arg_utils
//   task-032: op_mode
//   task-033: main_init
//   task-034: arg_parse
//   task-035: dispatch

pub mod constants;
pub mod help;
pub mod arg_utils;
pub mod op_mode;
pub mod init;
pub mod args;
