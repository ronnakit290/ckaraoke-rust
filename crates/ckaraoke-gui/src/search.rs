//! Typing to search without clicking a box, as the C++ app's search does. The rules are kept here, apart
//! from the window, so they can be tested.
//!
//! While the search has text, space types a space instead of pausing, and the arrow keys choose a song.
//! While it is empty, space pauses and the arrow keys seek.

/// Adds typed text to the search. Returns false if the text was not used. A space typed into an empty
/// search is not used, because space is the play/pause key.
pub fn accept_text(search: &mut String, typed: &str) -> bool {
    if typed == " " && search.is_empty() {
        return false;
    }
    let before = search.len();
    search.extend(typed.chars().filter(|c| !c.is_control()));
    search.len() != before
}

/// Removes the last character.
pub fn backspace(search: &mut String) {
    search.pop();
}

/// Moves the selection up or down through `len` results, staying inside them.
pub fn move_selection(selected: &mut usize, delta: isize, len: usize) {
    if len == 0 {
        *selected = 0;
        return;
    }
    let next = (*selected as isize + delta).clamp(0, len as isize - 1);
    *selected = next as usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_pauses_until_there_is_a_search() {
        let mut s = String::new();
        assert!(!accept_text(&mut s, " "));
        assert!(accept_text(&mut s, "คิด"));
        assert!(accept_text(&mut s, " "));
        assert_eq!(s, "คิด ");
    }

    #[test]
    fn control_characters_are_not_typed() {
        let mut s = String::from("a");
        assert!(!accept_text(&mut s, "\u{7f}"));
        assert_eq!(s, "a");
    }

    #[test]
    fn backspace_removes_one_character_including_thai_letters() {
        let mut s = String::from("คิด");
        backspace(&mut s);
        assert_eq!(s, "คิ");
        backspace(&mut String::new());
    }

    #[test]
    fn selection_stays_inside_the_results() {
        let mut sel = 0;
        move_selection(&mut sel, -1, 3);
        assert_eq!(sel, 0);
        move_selection(&mut sel, 5, 3);
        assert_eq!(sel, 2);
        move_selection(&mut sel, 1, 0);
        assert_eq!(sel, 0);
    }
}
