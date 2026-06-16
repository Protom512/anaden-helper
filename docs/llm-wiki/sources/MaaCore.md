# Source: MaaCore

## Overview
`MaaCore` is the core C++ library of MAA. It contains the image recognition logic, task orchestration, and device control.

## Directory Structure
- `Vision/`: Image recognition and template matching logic.
- `Task/`: Implementation of specific game tasks (Combat, Recruit, etc.).
- `Controller/`: Device interaction (ADB, Win32 API, etc.).
- `Common/`: Shared types and utilities.
- `Config/`: Configuration management.
- `Ui/`: UI-related core logic (not the GUI itself, but core UI abstractions).

## Key Files
- `Assistant.h/cpp`: The main entry point for the assistant logic.
- `AsstCaller.cpp`: Likely handles calling into the assistant from external bindings.
- `Status.h/cpp`: State management for the assistant.

---
[[LLM Wiki Index|Back to Index]]
