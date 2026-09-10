use cme_compiler::check::check;
use cme_compiler::parse_source;

fn show(source: &str) {
    let outcome = parse_source(source);
    if !outcome.diagnostics.is_empty() {
        let fe: Vec<String> = outcome.diagnostics.iter().map(|e| e.to_string()).collect();
        println!("FRONTEND {source:?} => {fe:?}");
        return;
    }
    let diags = check(&outcome.statements);
    let msgs: Vec<String> = diags.iter().map(|e| e.to_string()).collect();
    println!("{msgs:?} <= {source:?}");
}

fn main() {
    show("struct pair2<A, B> { A first\nB second }\nint f() {\nint x = 1\nreturn x\n}");
    show("int f() {\nmap<str, int> m = { 1: \"a\" }\nreturn 1\n}");
    show("int f() {\nint[] a = [1]\nreturn a[\"k\"]\n}");
    show("int f() {\nint x = 1\nreturn x[0]\n}");
    show("int f() {\nreturn 1.length\n}");
    show("int f() {\nint[] a = [1]\na.length = 3\nreturn 1\n}");
    show("int f() {\nint x = 1\nreturn x.foo\n}");
    show("int f() {\nfor (str v in [1, 2]) {\n}\nreturn 1\n}");
    show("int f() {\nfor (int v in 5) {\n}\nreturn 1\n}");
    show("int f() {\n1 + 1\nreturn 0\n}");
    show("int f() {\nint x = 1\nx\nreturn x\n}");
    show("struct s2 { int v }\nint f() { return 1 }\nimpl s2 { int v(s2 self) { return 1 } }");
    show("int f() { return 1 }\nimpl f { int m() { return 1 } }");
    show("struct s2 { int v }\nimpl s2 { void notAFunction(s2 self) { int q = 1 } }");
    show("impl option { int m() { return 1 } }");
    show("enum e2 { A(int v)\nB() }\nimpl e2 { int A(e2 self) { return 1 } }");
    show("int f(int v) {\nreturn v?\n}");
    show("result<int, str> f(result<int, str> r) {\nreturn Ok(r?)\n}");
    show("result<int, str> f(result<int, int> r) {\nreturn Ok(r?)\n}");
    show("int f(result<int, str> r) {\nreturn r?\n}");
    show("int f() {\nint a = 1\nfor (int i in [1]) {\n}\nreturn 1\n}");
    show("struct pair3<A> { int n }\nint f() {\npair3 p = pair3(n: 1)\nreturn 1\n}");
}
