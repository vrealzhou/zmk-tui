Feature: Edit and save a key binding
  As a keyboard user
  I want to replace a key binding from the terminal
  So that my keymap stays in sync with the keyboard

  Scenario: Replace Escape with A and save
    Given a simulated Corne keyboard is connected
    Then the editor shows "&kp ESCAPE"
    When I open the binding editor
    And I filter the behaviors by "key press"
    And I choose the first behavior
    And I filter the keycodes by "a"
    And I choose the first keycode
    Then the binding review is shown
    When I apply the binding
    Then the editor shows "&kp A"
    And the keymap is dirty
    When I save the keymap
    Then the keymap is clean
