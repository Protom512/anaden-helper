# Entity: Assistant

## Overview
`Assistant` is the primary class in `MaaCore` that implements the `AsstExtAPI`. It acts as the orchestrator for all MAA activities, managing device connections, task execution, and status reporting.

## Key Interface (`AsstExtAPI`)
- **Connection Management**:
    - `async_connect`: Connects to a device via ADB asynchronously.
    - `async_attach_window`: (Windows only) Binds to a Win32 window.
    - `connected()`: Checks if the assistant is connected to a device.
- **Task Management**:
    - `append_task(type, params)`: Adds a task (e.g., "Fight", "Infrast") to the queue with JSON parameters.
    - `start()`: Begins executing the task queue.
    - `stop()`: Stops task execution and clears the queue.
- **Device Interaction**:
    - `async_click(x, y)`: Performs a click at specific coordinates.
    - `async_screencap()`: Takes a screenshot.
    - `get_image()`: Retrieves the last screenshot.

## Internal Components
- `Controller`: Handles low-level device communication (ADB, Win32 input/screenshot).
- `Status`: Tracks the current state of the assistant.
- `InterfaceTask`: Represents a task in the queue.

## Concurrency
`Assistant` uses internal threads for:
- `call_proc()`: Processing asynchronous calls.
- `working_proc()`: Executing the task queue.
- `msg_proc()`: Handling callbacks and messages.

## Source
- `MaaCore/Assistant.h`
- `MaaCore/Assistant.cpp`

---
[[LLM Wiki Index|Back to Index]]
