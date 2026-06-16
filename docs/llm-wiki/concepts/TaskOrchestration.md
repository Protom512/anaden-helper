# Concept: Task Orchestration

## Overview
MAA organizes its automation logic into "Tasks". Each task represents a specific goal in the game, such as completing a combat stage or managing the infrastructure.

## Task Hierarchy

### Base Classes
- **AbstractTask**: The foundation of all tasks in MAA. It defines the lifecycle methods and common utilities.
- **ProcessTask**: Likely handles tasks that involve multiple sub-steps or processes.
- **InterfaceTask**: The external-facing task representation that is added to the `Assistant`'s task queue.

### Specific Tasks
- **Fight**: Handles combat stages, including sanity management and drop recognition.
- **Infrast**: Manages the base/infrastructure, including operator rotation and efficiency optimization.
- **Roguelike**: Fully automates the complex Roguelike game mode.
- **Recruit**: Automates recruitment, identifying high-rarity tags.
- **Reclamation**: Automates the "Reclamation Algorithm" game mode.
- **SSS**: Automates the "Stationary Security Service" game mode.

## Task Lifecycle
1. **Creation**: A task is instantiated (often via `append_task` in `Assistant`).
2. **Configuration**: Parameters (in JSON format) are passed to configure the task's behavior.
3. **Execution**: The `Assistant`'s worker thread calls the task's execution logic.
4. **Recognition & Action**: The task uses the `ImageRecognitionPipeline` to understand the screen and the `Controller` to perform clicks or swipes.
5. **Completion**: The task finishes and reports its status via callbacks.

## Source
- `MaaCore/Task/`

---
[[LLM Wiki Index|Back to Index]]
