Feature: Quit with unsaved changes
  The editor must not silently discard live keymap edits.

  Scenario: Discard an edit and quit
    Given a simulated Corne keyboard is connected
    When I open the binding editor
    And I filter the behaviors by "key press"
    And I choose the first behavior
    And I filter the keycodes by "b"
    And I choose the first keycode
    And I apply the binding
    Then the keymap is dirty
    When I request to quit
    Then the quit confirmation is shown
    When I discard the changes and quit
    Then the application should quit
