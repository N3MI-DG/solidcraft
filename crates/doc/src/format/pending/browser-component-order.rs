//! The browser's order of a component's placements: `browser_order` keeps, beside its folders'
//! orders, `<component>/components` (the placements' occurrence ids, as the tree lists them under
//! that component). It only orders the tree; placements and their names stay as they are. Older
//! designs need no transformation: without it, placements are listed in the order they were
//! made, as before. The last release refuses a design saved with this (the newer format number).
