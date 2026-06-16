# Concept: Image Recognition Pipeline

## Overview
MAA relies heavily on image recognition to understand the game state and perform actions. The vision system is modular and supports multiple matching algorithms.

## Key Components

### Matchers
- **Matcher**: The base class for image matching operations.
- **BestMatcher**: Likely finds the best match among several candidates.
- **FeatureMatcher**: Uses feature-based matching (e.g., SIFT, SURF, ORB) for robust recognition of complex scenes.
- **MaskedCcoeffMatcher**: Template matching with support for masks, allowing recognition of items with transparent or variable backgrounds.
- **MultiMatcher**: Handles matching multiple templates simultaneously.

### OCR (Optical Character Recognition)
- **OCRer**: The core OCR component, likely wrapping an external engine (like Tesseract) or a custom deep learning model.
- **RegionOCRer**: Specialized OCR for specific UI regions (e.g., resource counts, operator names).
- **TemplDetOCRer**: Template detection followed by OCR.

### Deep Learning
- **OnnxHelper**: Facilitates the use of ONNX models for recognition tasks, indicating that modern MAA versions may use deep learning for certain detection or classification problems.

## Workflow
1. **Screencap**: Get the current screen image from the `Controller`.
2. **Preprocessing**: (Optionally) crop, scale, or filter the image via `VisionHelper`.
3. **Matching/OCR**: Use a `Matcher` or `OCRer` to find specific elements or text.
4. **Decision**: The task logic uses the recognition results to determine the next action.

## Source
- `MaaCore/Vision/`

---
[[LLM Wiki Index|Back to Index]]
