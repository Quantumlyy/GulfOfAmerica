//! Built-in functions installed in every interpreter at startup.

use std::rc::Rc;

use crate::env::{self, Binding};
use crate::value::{BuiltinFn, Value};

use super::Interpreter;

pub fn install(interp: &mut Interpreter) {
    install_fn(interp, "print", |interp, args, _span| {
        let parts: Vec<String> = args.iter().map(Value::display).collect();
        let line = parts.join(" ");
        interp.output.borrow_mut().push_str(&line);
        interp.output.borrow_mut().push('\n');
        Ok(Value::Undefined)
    });
}

fn install_fn(
    interp: &mut Interpreter,
    name: &'static str,
    call: impl Fn(
            &mut Interpreter,
            Vec<Value>,
            crate::source::Span,
        ) -> Result<Value, crate::diagnostic::Diagnostic>
        + 'static,
) {
    let bf = BuiltinFn {
        name,
        call: Box::new(call),
    };
    let value = Value::BuiltinFn(Rc::new(bf));
    env::insert(
        &interp.globals,
        Binding {
            name: name.to_string(),
            value,
            decl: crate::ast::DeclKind::ConstConst,
            priority: i32::MIN, // builtins are always shadowable
            created_line: 0,
            created_at: interp.start_time,
            lifetime: Some(crate::ast::Lifetime::Infinity),
            eternal: true,
            previous_value: None,
        },
    );
}
