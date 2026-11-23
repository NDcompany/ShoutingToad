//! Accessibility checks measured in the rendered page (JavaScript rendering on).

use super::*;
use crate::render::Rendered;

fn tested(p: &Page) -> Option<&Rendered> {
    if !ih(p) {
        return None;
    }
    p.rendered.as_deref().filter(|r| r.a11y_tested)
}

pub fn add(v: &mut Vec<Check>) {
    const A: &str = "Accessibility";
    add!(v, "a11y.low_contrast", A, Issue, Medium, "Low Text Contrast",
        "Text whose colour contrast with its background is below the WCAG 1.4.3 (AA) minimum: 4.5:1, or 3:1 for large text. Measured from computed styles in the rendered page; text over images or gradients is skipped, not guessed.",
        "Darken the text or lighten the background until it reaches the minimum ratio. The inspector lists the colours and ratios.",
        |p, _| tested(p).is_some_and(|r| r.contrast_fail_count > 0));
    add!(v, "a11y.focus_not_visible", A, Warning, Medium, "No Visible Keyboard Focus",
        "Elements reached with the Tab key that look the same focused and unfocused (no outline, shadow, colour or position change), often from outline:none. Keyboard users cannot see where they are (WCAG 2.4.7).",
        "Keep or add a clear :focus-visible style, such as an outline, on links, buttons and form fields.",
        |p, _| tested(p).is_some_and(|r| !r.focus_not_visible.is_empty()));
    add!(v, "a11y.click_not_keyboard", A, Warning, Medium, "Click Handlers Not Reachable by Keyboard",
        "Elements such as <div> or <span> with mouse click handlers that cannot get keyboard focus, so keyboard and switch users cannot use them (WCAG 2.1.1). Handlers delegated to the page root are not detected.",
        "Use a <button> or <a href> instead, or add tabindex=\"0\", a role and key handlers.",
        |p, _| tested(p).is_some_and(|r| !r.click_no_keyboard.is_empty()));
}
