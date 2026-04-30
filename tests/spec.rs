//! Spec-mirror tests.
//!
//! Every test below corresponds to a code block in the upstream Gulf of Mexico
//! / DreamBerd README. The expected output is taken from the `// comment`
//! that the README pairs with that example. When the README does not specify
//! an expected output (e.g. a declaration that does no I/O), the test asserts
//! that running the program produces no output.
//!
//! These tests act as the *contract* that the interpreter must satisfy. They
//! were written before the lexer/parser/interpreter were implemented and were
//! used to drive their development.
//!
//! README source: <https://github.com/TodePond/GulfOfMexico/blob/main/README.md>

use gulf::run;

/// Helper: run a program and return its captured stdout (without trailing
/// newline normalisation — `print` always emits a `\n`).
fn out(src: &str) -> String {
    match run(src, "spec.gom") {
        Ok(s) => s,
        Err(e) => panic!("expected program to succeed, but got:\n{e}\n--- source ---\n{src}"),
    }
}

/// Helper: run a program and assert it fails with a diagnostic containing all
/// of the substrings in `needles`.
fn err_contains(src: &str, needles: &[&str]) -> String {
    match run(src, "spec.gom") {
        Ok(s) => panic!("expected program to fail, but it produced:\n{s}\n--- source ---\n{src}"),
        Err(e) => {
            for n in needles {
                assert!(
                    e.contains(n),
                    "expected error to contain {n:?}, got:\n{e}\n--- source ---\n{src}"
                );
            }
            e
        }
    }
}

// ---------------------------------------------------------------------------
// § Exclamation Marks
// ---------------------------------------------------------------------------

#[test]
fn readme_print_hello_world_with_one_bang() {
    assert_eq!(out(r#"print("Hello world")!"#), "Hello world\n");
}

#[test]
fn readme_print_hello_world_with_three_bangs() {
    // Multiple `!`s are still a perfectly valid statement terminator.
    assert_eq!(out(r#"print("Hello world")!!!"#), "Hello world\n");
}

#[test]
fn readme_question_mark_prints_debug_info_for_the_line() {
    // `?` runs the statement *and* emits a debug line. The exact debug
    // formatting is up to us, but it must (a) include the source of the
    // expression and (b) include the resulting value, and (c) the program
    // must still produce the side-effect output of the underlying call.
    let s = out(r#"print("Hello world")?"#);
    assert!(s.contains("Hello world\n"), "debug should not eat the print output: {s:?}");
    assert!(s.contains("print"), "debug should mention the source: {s:?}");
}

#[test]
fn readme_semicolon_is_the_not_operator() {
    let src = r#"
if (;false) {
   print("Hello world")!
}
"#;
    assert_eq!(out(src), "Hello world\n");
}

// ---------------------------------------------------------------------------
// § Declarations
// ---------------------------------------------------------------------------

#[test]
fn readme_const_const_is_fully_immutable() {
    // The README only declares the variable; we extend it minimally to
    // observe the binding.
    let src = r#"
const const name = "Luke"!
print(name)!
"#;
    assert_eq!(out(src), "Luke\n");
}

#[test]
fn readme_const_const_cannot_be_reassigned() {
    err_contains(
        r#"
const const name = "Luke"!
name = "Lu"!
"#,
        &["const const", "reassign"],
    );
}

#[test]
fn readme_const_var_can_be_edited_but_not_reassigned() {
    // The README's example calls `name.pop()` twice on `"Luke"`. Strings are
    // arrays of chars, so two pops yields "Lu".
    let src = r#"
const var name = "Luke"!
name.pop()!
name.pop()!
print(name)!
"#;
    assert_eq!(out(src), "Lu\n");
}

#[test]
fn readme_const_var_cannot_be_reassigned() {
    err_contains(
        r#"
const var name = "Luke"!
name = "Lu"!
"#,
        &["const var", "reassign"],
    );
}

#[test]
fn readme_var_const_can_be_reassigned() {
    let src = r#"
var const name = "Luke"!
name = "Lu"!
print(name)!
"#;
    assert_eq!(out(src), "Lu\n");
}

#[test]
fn readme_var_const_cannot_be_mutated() {
    err_contains(
        r#"
var const name = "Luke"!
name.pop()!
"#,
        &["var const", "mutate"],
    );
}

#[test]
fn readme_var_var_can_be_reassigned_and_mutated() {
    let src = r#"
var var name = "Luke"!
name = "Lu"!
name.push("k")!
name.push("e")!
print(name)!
"#;
    assert_eq!(out(src), "Luke\n");
}

// ---------------------------------------------------------------------------
// § Immutable Data
// ---------------------------------------------------------------------------

#[test]
fn readme_const_const_const_pi() {
    let src = r#"
const const const pi = 3.14!
print(pi)!
"#;
    assert_eq!(out(src), "3.14\n");
}

// ---------------------------------------------------------------------------
// § Naming
// ---------------------------------------------------------------------------

#[test]
fn readme_redefining_the_number_5_to_be_4() {
    // From the README:
    //   const const 5 = 4!
    //   print(2 + 2 === 5)! //true
    let src = r#"
const const 5 = 4!
print(2 + 2 === 5)!
"#;
    assert_eq!(out(src), "true\n");
}

// ---------------------------------------------------------------------------
// § Arrays
// ---------------------------------------------------------------------------

#[test]
fn readme_arrays_start_at_negative_one() {
    let src = r#"
const const scores = [3, 2, 5]!
print(scores[-1])!
print(scores[0])!
print(scores[1])!
"#;
    assert_eq!(out(src), "3\n2\n5\n");
}

#[test]
fn readme_float_index_inserts_mid_array() {
    // From the README:
    //   const var scores = [3, 2, 5]!
    //   scores[0.5] = 4!
    //   print(scores)! //[3, 2, 4, 5]
    let src = r#"
const var scores = [3, 2, 5]!
scores[0.5] = 4!
print(scores)!
"#;
    assert_eq!(out(src), "[3, 2, 4, 5]\n");
}

// ---------------------------------------------------------------------------
// § When
// ---------------------------------------------------------------------------

#[test]
fn readme_when_triggers_on_mutation() {
    // The README declares `const var health = 10!` and watches it. To
    // actually demonstrate the trigger we need to reassign `health`, which
    // requires `var var` (the README's example never performs the
    // assignment and thus never fires the watcher).
    let src = r#"
var var health = 10!
when (health = 0) {
   print("You lose")!
}
health = 0!
"#;
    assert_eq!(out(src), "You lose\n");
}

#[test]
fn readme_when_does_not_trigger_when_condition_is_false() {
    let src = r#"
var var health = 10!
when (health = 0) {
   print("You lose")!
}
health = 5!
print("still alive")!
"#;
    assert_eq!(out(src), "still alive\n");
}

// ---------------------------------------------------------------------------
// § Lifetimes
// ---------------------------------------------------------------------------

#[test]
fn readme_two_line_lifetime_expires() {
    // The variable lives for two lines after declaration. On the third line,
    // it has expired.
    let src = r#"
const const name<2> = "Luke"!
print(name)!
print("after")!
"#;
    let s = out(src);
    // First print sees the value; second print is past the lifetime so
    // accessing `name` would error — but we don't access it. The point of
    // this test is just that the program runs.
    assert!(s.contains("Luke"), "should print Luke once: {s}");
    assert!(s.contains("after"), "after-line should run: {s}");
}

#[test]
fn readme_negative_lifetime_hoists_the_binding() {
    // Variable hoisting via negative lifetime — the variable exists *before*
    // its declaration and disappears at it.
    let src = r#"
print(name)!
const const name<-1> = "Luke"!
"#;
    assert_eq!(out(src), "Luke\n");
}

// ---------------------------------------------------------------------------
// § Booleans
// ---------------------------------------------------------------------------

#[test]
fn readme_three_valued_boolean_maybe_is_printable() {
    let src = r#"
const const m = maybe!
print(m)!
"#;
    assert_eq!(out(src), "maybe\n");
}

// ---------------------------------------------------------------------------
// § Arithmetic — significant whitespace
// ---------------------------------------------------------------------------

#[test]
fn readme_arithmetic_whitespace_precedence_one_plus_two_times_three_is_seven() {
    // README: print(1 + 2*3)! //7
    assert_eq!(out("print(1 + 2*3)!"), "7\n");
}

#[test]
fn readme_arithmetic_whitespace_precedence_oneplustwo_times_three_is_nine() {
    // README: print(1+2 * 3)! //9
    assert_eq!(out("print(1+2 * 3)!"), "9\n");
}

#[test]
fn readme_arithmetic_supports_fractions() {
    let src = r#"
const const half = 1/2!
print(half)!
"#;
    assert_eq!(out(src), "0.5\n");
}

#[test]
fn readme_arithmetic_number_names() {
    // README: print(one + two)! //3
    assert_eq!(out("print(one + two)!"), "3\n");
}

// ---------------------------------------------------------------------------
// § Equality
// ---------------------------------------------------------------------------

#[test]
fn readme_loose_equality_two_equals() {
    // README: 3.14 == "3.14"! //true
    assert_eq!(out(r#"print(3.14 == "3.14")!"#), "true\n");
}

#[test]
fn readme_strict_equality_three_equals() {
    // README: 3.14 === "3.14"! //false
    assert_eq!(out(r#"print(3.14 === "3.14")!"#), "false\n");
}

#[test]
fn readme_extreme_equality_pi_eqeqeqeq_pi_is_true() {
    let src = r#"
const const pi = 3.14!
print(pi ==== pi)!
"#;
    assert_eq!(out(src), "true\n");
}

#[test]
fn readme_extreme_equality_literal_to_literal_is_true() {
    // README: print(3.14 ==== 3.14)! //true
    assert_eq!(out("print(3.14 ==== 3.14)!"), "true\n");
}

#[test]
fn readme_extreme_equality_literal_to_variable_is_false() {
    // README: print(3.14 ==== pi)! //false
    let src = r#"
const const pi = 3.14!
print(3.14 ==== pi)!
"#;
    assert_eq!(out(src), "false\n");
}

#[test]
fn readme_least_precise_equality_one_equals() {
    // README: 3 = 3.14! //true  (interpreted as comparison since `3` is not a
    // declaration target here.)
    assert_eq!(out("print(3 = 3.14)!"), "true\n");
}

// ---------------------------------------------------------------------------
// § Functions — every spelling of "function"
// ---------------------------------------------------------------------------

#[test]
fn readme_function_keyword_function() {
    assert_eq!(
        out("function add(a, b) => a + b!\nprint(add(3, 2))!"),
        "5\n"
    );
}

#[test]
fn readme_function_keyword_func() {
    assert_eq!(
        out("func multiply(a, b) => a * b!\nprint(multiply(3, 2))!"),
        "6\n"
    );
}

#[test]
fn readme_function_keyword_fun() {
    assert_eq!(
        out("fun subtract(a, b) => a - b!\nprint(subtract(3, 2))!"),
        "1\n"
    );
}

#[test]
fn readme_function_keyword_fn() {
    assert_eq!(
        out("fn divide(a, b) => a / b!\nprint(divide(6, 2))!"),
        "3\n"
    );
}

#[test]
fn readme_function_keyword_f() {
    assert_eq!(out("f inverse(a) => 1/a!\nprint(inverse(2))!"), "0.5\n");
}

// ---------------------------------------------------------------------------
// § Dividing by Zero
// ---------------------------------------------------------------------------

#[test]
fn readme_dividing_by_zero_returns_undefined() {
    // README: print(3 / 0)! //undefined
    assert_eq!(out("print(3 / 0)!"), "undefined\n");
}

// ---------------------------------------------------------------------------
// § Strings
// ---------------------------------------------------------------------------

#[test]
fn readme_strings_single_quote() {
    assert_eq!(out("print('Lu')!"), "Lu\n");
}

#[test]
fn readme_strings_double_quote() {
    assert_eq!(out(r#"print("Luke")!"#), "Luke\n");
}

#[test]
fn readme_strings_triple_quote() {
    assert_eq!(out("print('''Lu''')!"), "Lu\n");
}

#[test]
fn readme_strings_quotes_inside_single_outer() {
    // The README writes  const const name = "'Lu'"!  — the inner single
    // quotes are content.
    assert_eq!(out(r#"print("'Lu'")!"#), "'Lu'\n");
}

#[test]
fn readme_strings_quadruple_quote() {
    // README: const const name = """"Luke""""!
    assert_eq!(out(r#"print(""""Luke"""")!"#), "Luke\n");
}

#[test]
fn readme_strings_zero_quote_bareword() {
    // README: const const name = Luke!
    let src = r#"
const const greeting = Luke!
print(greeting)!
"#;
    assert_eq!(out(src), "Luke\n");
}

// ---------------------------------------------------------------------------
// § String Interpolation — currencies
// ---------------------------------------------------------------------------

#[test]
fn readme_interpolation_us_dollar() {
    let src = r#"
const const name = "world"!
print("Hello ${name}!")!
"#;
    assert_eq!(out(src), "Hello world!\n");
}

#[test]
fn readme_interpolation_uk_pound() {
    let src = r#"
const const name = "world"!
print("Hello £{name}!")!
"#;
    assert_eq!(out(src), "Hello world!\n");
}

#[test]
fn readme_interpolation_japanese_yen() {
    let src = r#"
const const name = "world"!
print("Hello ¥{name}!")!
"#;
    assert_eq!(out(src), "Hello world!\n");
}

#[test]
fn readme_interpolation_trailing_euro() {
    // README: print("Hello {name}€!")!
    let src = r#"
const const name = "world"!
print("Hello {name}€!")!
"#;
    assert_eq!(out(src), "Hello world!\n");
}

// ---------------------------------------------------------------------------
// § Types — annotations are parsed and ignored
// ---------------------------------------------------------------------------

#[test]
fn readme_type_annotations_are_ignored() {
    let src = r#"
const var age: Int = 28!
print(age)!
"#;
    assert_eq!(out(src), "28\n");
}

#[test]
fn readme_int9_int99_are_just_integers() {
    let src = r#"
const var age: Int9 = 28!
print(age)!
"#;
    assert_eq!(out(src), "28\n");
}

// ---------------------------------------------------------------------------
// § File Structure
// ---------------------------------------------------------------------------

#[test]
fn readme_file_separator_resets_environment() {
    let src = r#"
const const score = 5!
print(score)!

=====================

const const score = 3!
print(score)!
"#;
    assert_eq!(out(src), "5\n3\n");
}

// ---------------------------------------------------------------------------
// § Classes — single instance restriction
// ---------------------------------------------------------------------------

#[test]
fn readme_classes_single_instance_is_allowed() {
    let src = r#"
class Player {
   const var health = 10!
}
const var player1 = new Player()!
print(player1.health)!
"#;
    assert_eq!(out(src), "10\n");
}

#[test]
fn readme_classes_second_instance_errors() {
    // README: const var player2 = new Player()! //Error: Can't have more than
    // one 'Player' instance!
    err_contains(
        r#"
class Player {
   const var health = 10!
}
const var player1 = new Player()!
const var player2 = new Player()!
"#,
        &["Player", "instance"],
    );
}

// ---------------------------------------------------------------------------
// § Delete
// ---------------------------------------------------------------------------

#[test]
fn readme_delete_a_primitive() {
    err_contains(
        r#"
delete 3!
print(2 + 1)!
"#,
        &["3", "delete"],
    );
}

// ---------------------------------------------------------------------------
// § Overloading
// ---------------------------------------------------------------------------

#[test]
fn readme_overloading_most_recent_wins() {
    // README:
    //   const const name = "Luke"!
    //   const const name = "Lu"!
    //   print(name)! // "Lu"
    let src = r#"
const const name = "Luke"!
const const name = "Lu"!
print(name)!
"#;
    assert_eq!(out(src), "Lu\n");
}

#[test]
fn readme_overloading_more_bangs_wins_over_recency() {
    let src = r#"
const const name = "Lu"!!
const const name = "Luke"!
print(name)!
"#;
    assert_eq!(out(src), "Lu\n");
}

#[test]
fn readme_overloading_many_bangs_dominates() {
    let src = r#"
const const name = "Lu or Luke (either is fine)"!!!!!!!!!
print(name)!
"#;
    assert_eq!(out(src), "Lu or Luke (either is fine)\n");
}

#[test]
fn readme_overloading_inverted_bang_is_negative_priority() {
    let src = r#"
const const name = "Lu"!
const const name = "Luke"¡
print(name)!
"#;
    assert_eq!(out(src), "Lu\n");
}

// ---------------------------------------------------------------------------
// § Parentheses — they are pure whitespace
// ---------------------------------------------------------------------------

#[test]
fn readme_parens_are_whitespace_normal() {
    let src = r#"
function add(a, b) => a + b!
print(add(3, 2))!
"#;
    assert_eq!(out(src), "5\n");
}

#[test]
fn readme_parens_are_whitespace_no_parens_at_all() {
    // README form: `add 3, 2!` — no parens at all. Parens being whitespace
    // means that nested calls (`print(add(3, 2))`) still need *some* paren
    // to mark the boundary, but a single non-nested call without parens
    // works fine.
    let src = r#"
function add(a, b) => {
   print(a + b)!
}
add 3, 2!
"#;
    assert_eq!(out(src), "5\n");
}

#[test]
fn readme_parens_are_whitespace_extra_parens() {
    // README: (add (3, 2))!
    let src = r#"
function add(a, b) => {
   print(a + b)!
}
(add (3, 2))!
"#;
    assert_eq!(out(src), "5\n");
}

// ---------------------------------------------------------------------------
// § Indentation — 3 spaces is canonical, but the spec calls -3 spaces "also
// allowed". We accept both forms (and arbitrary indentation) — the language
// is whitespace-significant only inside expressions.
// ---------------------------------------------------------------------------

#[test]
fn readme_indents_three_spaces() {
    let src = "\
function main() => {
   print(\"Gulf of Mexico is the future\")!
}
main()!
";
    assert_eq!(out(src), "Gulf of Mexico is the future\n");
}

// ---------------------------------------------------------------------------
// § Diagnostics — the language may be a meme, but its errors should not be.
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_unterminated_string_points_at_the_open_quote() {
    let e = err_contains(r#"print("Hello)!"#, &["string"]);
    assert!(
        e.contains("spec.gom:"),
        "diagnostic must include source location: {e}"
    );
}

#[test]
fn diagnostic_calling_a_non_function_errors() {
    // The language has bareword strings, so a plain identifier evaluates to
    // its name when undefined. Calling that string is what should error.
    err_contains(
        r#"some_undefined_thing(1, 2)!"#,
        &["string", "call"],
    );
}

#[test]
fn diagnostic_use_after_lifetime_expiry() {
    let src = r#"
const const name<1> = "Luke"!
print("filler")!
print(name)!
"#;
    err_contains(src, &["name", "expired"]);
}

// ---------------------------------------------------------------------------
// § Signals — `use(initial)` returns a getter/setter pair.
// ---------------------------------------------------------------------------

#[test]
fn signals_destructured_pair_round_trips() {
    let src = r#"
const var [getScore, setScore] = use(0)!
print(getScore())!
setScore(7)!
print(getScore())!
"#;
    assert_eq!(out(src), "0\n7\n");
}

#[test]
fn signals_setter_visible_through_getter() {
    let src = r#"
const var [count, setCount] = use(10)!
setCount(count() + 1)!
setCount(count() + 1)!
print(count())!
"#;
    assert_eq!(out(src), "12\n");
}

#[test]
fn signals_callable_directly_when_not_destructured() {
    // A non-destructured signal binding is itself callable: zero args reads,
    // one arg writes.
    let src = r#"
const var sig = use(5)!
print(sig())!
sig(99)!
print(sig())!
"#;
    assert_eq!(out(src), "5\n99\n");
}

// ---------------------------------------------------------------------------
// § previous / next / current
// ---------------------------------------------------------------------------

#[test]
fn time_current_returns_current_value() {
    let src = r#"
var var name = "Luke"!
print(current name)!
"#;
    assert_eq!(out(src), "Luke\n");
}

#[test]
fn time_previous_returns_value_before_last_reassignment() {
    let src = r#"
var var name = "Luke"!
name = "Lu"!
print(previous name)!
print(name)!
"#;
    assert_eq!(out(src), "Luke\nLu\n");
}

#[test]
fn time_previous_falls_back_to_current_when_never_reassigned() {
    let src = r#"
var var name = "Luke"!
print(previous name)!
"#;
    assert_eq!(out(src), "Luke\n");
}

#[test]
fn time_next_peeks_at_upcoming_assignment() {
    let src = r#"
var var name = "Luke"!
print(next name)!
name = "Lu"!
print(name)!
"#;
    assert_eq!(out(src), "Lu\nLu\n");
}

// ---------------------------------------------------------------------------
// § Async functions — line-interleaved execution
// ---------------------------------------------------------------------------

#[test]
fn async_un_awaited_call_interleaves_with_main_thread() {
    // The body of `tick()` runs one statement per main-thread statement.
    let src = r#"
async function tick() => {
   print("a")!
   print("b")!
}
print("1")!
tick()!
print("2")!
print("3")!
"#;
    // After the `tick()` call: main has emitted "1", task queued.
    // After `print("2")`: main "2", tick fires once: "a".
    // After `print("3")`: main "3", tick fires again: "b".
    assert_eq!(out(src), "1\na\n2\nb\n3\n");
}

#[test]
fn async_await_runs_synchronously_and_returns_value() {
    let src = r#"
async function compute() => {
   return 42!
}
const const result = await compute()!
print(result)!
"#;
    assert_eq!(out(src), "42\n");
}

#[test]
fn async_drains_pending_after_main_thread_finishes() {
    // The main thread issues only two statements; the queued `task` body has
    // three. After every main statement (including the call itself) the
    // pending tasks tick once, so the trailing two ticks happen during the
    // post-main drain.
    let src = r#"
async function task() => {
   print("first")!
   print("second")!
   print("third")!
}
task()!
print("main done")!
"#;
    assert_eq!(out(src), "first\nmain done\nsecond\nthird\n");
}

// ---------------------------------------------------------------------------
// § reverse! — flips the remaining statements in the file.
// ---------------------------------------------------------------------------

#[test]
fn reverse_flips_remaining_statements() {
    let src = r#"
print("a")!
print("b")!
reverse!
print("c")!
print("d")!
"#;
    assert_eq!(out(src), "a\nb\nd\nc\n");
}

#[test]
fn reverse_at_program_start_runs_everything_in_reverse() {
    let src = r#"
reverse!
print("a")!
print("b")!
print("c")!
"#;
    assert_eq!(out(src), "c\nb\na\n");
}

// ---------------------------------------------------------------------------
// § export … to / import — plumbing values between `=====`-separated files.
// ---------------------------------------------------------------------------

#[test]
fn export_to_main_imports_function() {
    let src = r#"
function add(a, b) => a + b!
export add to "main.gom"!
===== main.gom =====
import add!
print(add(3, 2))!
"#;
    assert_eq!(out(src), "5\n");
}

#[test]
fn export_to_main_imports_value() {
    let src = r#"
const const greeting = "hi"!
export greeting to "main.gom"!
===== main.gom =====
import greeting!
print(greeting)!
"#;
    assert_eq!(out(src), "hi\n");
}

#[test]
fn import_without_matching_export_errors() {
    err_contains(
        r#"
===== main.gom =====
import nope!
print(nope)!
"#,
        &["import", "nope"],
    );
}

// ---------------------------------------------------------------------------
// § std http — `import http!` binds a module with client + server primitives.
// ---------------------------------------------------------------------------

#[test]
fn http_module_binds_via_stdlib_import() {
    // Even with no `=====`-separated peer file exporting `http`, the import
    // succeeds because `http` is a known std package.
    let src = r#"
import http!
print(http.get)!
"#;
    let s = out(src);
    assert!(
        s.contains("builtin"),
        "expected http.get to be a builtin function, got: {s:?}"
    );
}

#[test]
fn http_unknown_std_name_still_errors() {
    // `nope` is not in the stdlib registry, so the error should match the
    // pre-existing missing-export diagnostic.
    err_contains(
        r#"
import nope!
"#,
        &["import", "nope"],
    );
}

#[test]
fn http_user_export_shadows_stdlib() {
    // If a peer file exports a binding called `http`, the user wins. This
    // keeps the stdlib registry from quietly stealing user names.
    let src = r#"
const const http = "the user value"!
export http to "main.gom"!
===== main.gom =====
import http!
print(http)!
"#;
    assert_eq!(out(src), "the user value\n");
}

#[test]
fn http_serve_once_roundtrip() {
    // End-to-end smoke test: spawn a real serve_once on a free port, fire a
    // GET at it from a Rust client thread, assert the response.
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    let src = format!(
        r#"
import http!
function handle(req) => {{
   return {{status: 201, body: "got " + req.method + " " + req.path}}!
}}
http.serve_once("127.0.0.1:{port}", handle)!
print("done")!
"#
    );

    let server = thread::spawn(move || gulf::run(&src, "server.gom"));

    // Give the server a moment to bind. We retry the connect a few times to
    // tolerate the small race between bind and accept.
    let mut stream = None;
    for _ in 0..50 {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
            stream = Some(s);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let mut stream = stream.expect("could not connect to test server");
    stream
        .write_all(b"GET /spec HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();

    assert!(response.starts_with("HTTP/1.1 201"), "response: {response}");
    assert!(response.contains("got GET /spec"), "response: {response}");

    let server_out = server.join().unwrap().expect("server program failed");
    assert!(server_out.contains("done\n"), "server output: {server_out:?}");
}

#[test]
fn http_client_get_against_local_listener() {
    // The mirror of the previous test: spin up a *Rust* TCP listener that
    // serves a canned response, then drive `http.get` from inside the
    // interpreter and confirm the parsed shape.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let body = "pong";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
            body.len(),
            body,
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let src = format!(
        r#"
import http!
const const res = http.get("http://127.0.0.1:{port}/")!
print(res.status)!
print(res.body)!
print(res.headers["content-type"])!
"#
    );
    let s = out(&src);
    server.join().unwrap();

    assert_eq!(s, "200\npong\ntext/plain\n");
}

#[test]
fn http_post_sends_body() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap();
        let received = String::from_utf8_lossy(&buf[..n]).into_owned();
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        stream.write_all(response.as_bytes()).unwrap();
        received
    });

    let src = format!(
        r#"
import http!
const const res = http.post("http://127.0.0.1:{port}/echo", "payload-123")!
print(res.body)!
"#
    );
    let s = out(&src);
    let received = server.join().unwrap();

    assert_eq!(s, "ok\n");
    assert!(
        received.starts_with("POST /echo HTTP/1.1\r\n"),
        "wire: {received}"
    );
    assert!(received.contains("Content-Length: 11"), "wire: {received}");
    assert!(received.ends_with("payload-123"), "wire: {received}");
}

#[test]
fn http_get_rejects_https_urls() {
    err_contains(
        r#"
import http!
http.get("https://example.com")!
"#,
        &["only http://"],
    );
}

#[test]
fn http_successful_response_includes_ok_true() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";
        stream.write_all(response.as_bytes()).unwrap();
    });

    let src = format!(
        r#"
import http!
const const res = http.get("http://127.0.0.1:{port}/")!
print(res.ok)!
print(res.status)!
"#
    );
    let s = out(&src);
    server.join().unwrap();
    assert_eq!(s, "true\n200\n");
}

#[test]
fn http_connect_failure_returns_ok_false_instead_of_aborting() {
    // Port 1 should refuse a TCP connection on most systems. We intentionally
    // never bind it: the test asserts that `http.get` against an unreachable
    // address returns a result-shaped value with `ok: false`, lets the rest
    // of the program run, and exposes a non-empty `error` field.
    let src = r#"
import http!
const const res = http.get("http://127.0.0.1:1/")!
print(res.ok)!
print(res.status)!
print(res.error.length() > 0)!
print("after")!
"#;
    let s = out(src);
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 4, "output: {s:?}");
    assert_eq!(lines[0], "false");
    assert_eq!(lines[1], "0");
    assert_eq!(lines[2], "true");
    assert_eq!(lines[3], "after");
}

#[test]
fn http_decodes_chunked_response_body() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        // Three chunks: "Wiki", "pedia", " in chunks." then terminator.
        let response = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                        4\r\nWiki\r\n\
                        5\r\npedia\r\n\
                        b\r\n in chunks.\r\n\
                        0\r\n\r\n";
        stream.write_all(response.as_bytes()).unwrap();
    });

    let src = format!(
        r#"
import http!
const const res = http.get("http://127.0.0.1:{port}/")!
print(res.body)!
"#
    );
    let s = out(&src);
    server.join().unwrap();
    assert_eq!(s, "Wikipedia in chunks.\n");
}

// ---------------------------------------------------------------------------
// § Parser recovery — multiple errors per pass.
// ---------------------------------------------------------------------------

#[test]
fn parser_reports_every_error_in_one_pass() {
    use gulf::{lexer, parser, source::SourceFile};

    let src = "\
print(1)
const const x = !
print(2)!
class
function 3 => 4!
print(3)!
";
    let file = SourceFile::new("multi.gom".into(), src.into());
    let tokens = lexer::lex(&file).expect("lexer should succeed");
    let (program, diags) = parser::parse_recovering(&file, tokens);
    assert!(
        diags.len() >= 2,
        "expected multiple parse diagnostics, got {}: {:#?}",
        diags.len(),
        diags.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
    // The recovering parser should have salvaged at least one good statement
    // — concretely, the `print(2)!` that follows the broken `const const`.
    let total_stmts: usize = program.files.iter().map(|f| f.stmts.len()).sum();
    assert!(
        total_stmts >= 1,
        "expected at least one statement to survive recovery"
    );
}
