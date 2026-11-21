Feature: Layers and physical boards
  Scenario: Switch to a layer with the numeric shortcut
    Given a simulated Corne keyboard is connected
    When I switch to layer 2
    Then the editor shows "2 Raise"

  Scenario: Sofle exposes five layers and sixty positions
    Given a simulated Sofle keyboard is connected
    Then the editor shows "4 Media"
    And the editor shows "Sofle"
