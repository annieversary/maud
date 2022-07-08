use maud::html;

#[test]
fn issue_170() {
    let number = 42;
    let _ = html! { (number) };
}
