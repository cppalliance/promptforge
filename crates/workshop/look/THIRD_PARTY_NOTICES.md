# Third-Party Notices

Source in this directory that derives from another project, with the notice its license requires.

## Codicons

`icons.ts` holds the artwork of 28 icons from the Visual Studio Code codicon set - add, agent, arrow-up, ask, bug, check, checklist, chevron-down, chevron-right, close, copy, ellipsis, error, file, file-code, file-media, file-text, folder, globe, info, layers, link, mic, new-folder, stop-circle, symbol-keyword, trash, and warning - as inline SVG strings. Each string is the icon's `src/icons/<name>.svg` file from `@vscode/codicons` with only the `width` and `height` attributes changed. `@vscode/codicons` is a `devDependency` that only `test/icons.mjs` reads, to check every string against its source file.

- Project: <https://github.com/microsoft/vscode-codicons>
- Package: `@vscode/codicons` 0.0.46-24
- Copyright: (c) Microsoft Corporation
- License: Creative Commons Attribution 4.0 International (CC-BY-4.0), <https://creativecommons.org/licenses/by/4.0/legalcode>. The codicon font and icon artwork are licensed CC-BY-4.0; the package's code is MIT.
- Changes: the `width` and `height` attributes of each string are set to the size the consumer needs. The paths are unmodified.

## murm-ui

`dropdown.ts` and `dropdown.css` are ported from the `components/dropdown.ts` and `styles/dropdown.css` files of murm-ui 0.2.0 (commit `336ff7db79d928373e83c3672db6041a0adbc868`), cut to the shared action-menu's needs and restyled onto the Cursor Dark tokens. (Moved here from the workshop UI package, now `crates/workshop/ui/src/parts/`, where the port first landed.)

- Project: <https://github.com/levmv/murm-ui>
- License: MIT

```text
MIT License

Copyright (c) 2026 Lev Morozov

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
