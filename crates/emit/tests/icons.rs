//! Inventory icons and the item art pair: icons fit the box, parse back
//! through `FrmArchive::parse`, and both art entries resolve through the
//! engine's art catalog. All pictures are synthetic.

use emit::icon::{
    INVENTORY_CATEGORY, INVENTORY_ICON_MAX, ITEM_CATEGORY, fit_size, inventory_frm_indexed,
    inventory_frm_rgba, push_item_art,
};
use emit::{ArtList, EmitError, FormatWriters, LocalWriters};
use vault_formats::FrmArchive;

fn picture(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0_u8; (width * height) as usize];
    for y in 2..height - 2 {
        for x in 3..width - 3 {
            pixels[(y * width + x) as usize] = 1 + ((x + 2 * y) % 250) as u8;
        }
    }
    pixels
}

#[test]
fn a_small_icon_keeps_its_pixels_and_a_large_one_fits_the_box() {
    let pixels = picture(40, 30);
    let spec = inventory_frm_indexed(&pixels, 40, 30, INVENTORY_ICON_MAX).unwrap();
    let archive = FrmArchive::parse(&LocalWriters.frm(&spec).unwrap()).unwrap();
    assert_eq!(archive.frame_count(), 1);
    let frame = archive.frame(0, 0).unwrap();
    // Cropped to the opaque 34 x 26, unscaled.
    assert_eq!((frame.width, frame.height), (34, 26));
    assert_eq!(frame.pixels()[0], pixels[2 * 40 + 3]);
    assert_eq!(archive.shifts[0], [0, 0]);
    for d in 1..6 {
        assert_eq!(archive.frame(d, 0).unwrap().pixels(), frame.pixels());
    }

    let large = picture(400, 100);
    let spec = inventory_frm_indexed(&large, 400, 100, INVENTORY_ICON_MAX).unwrap();
    let frame = &spec.directions[0].frames[0];
    assert!(u32::from(frame.width) <= 90 && u32::from(frame.height) <= 61);
    assert_eq!(frame.width, 90); // the width limits a 394 x 96 picture
    FrmArchive::parse(&LocalWriters.frm(&spec).unwrap()).unwrap();

    assert_eq!(fit_size(200, 100, [90, 61]), [90, 45]);
    assert_eq!(fit_size(50, 200, [90, 61]), [15, 61]);
    assert_eq!(fit_size(10, 10, [90, 61]), [10, 10]);
    assert_eq!(fit_size(0, 10, [90, 61]), [0, 0]);
    assert!(inventory_frm_indexed(&[0; 16], 4, 4, INVENTORY_ICON_MAX).is_err());
    assert!(inventory_frm_indexed(&[1; 15], 4, 4, INVENTORY_ICON_MAX).is_err());
    assert!(inventory_frm_indexed(&[1; 16], 4, 4, [0, 10]).is_err());
}

#[test]
fn rgba_icons_average_then_quantize() {
    let (width, height) = (180, 122);
    let mut rgba = vec![0_u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            let red = if x < 90 { 200 } else { 40 };
            rgba[at..at + 4].copy_from_slice(&[red, 100, 50, 255]);
        }
    }
    let quantize = |c: [u8; 3]| if c[0] > 120 { 7 } else { 9 };
    let spec = inventory_frm_rgba(
        &rgba,
        width as u32,
        height as u32,
        INVENTORY_ICON_MAX,
        &quantize,
    )
    .unwrap();
    let frame = &spec.directions[0].frames[0];
    // 180 x 122 opaque, halved to the 90 x 61 box.
    assert_eq!((frame.width, frame.height), (90, 61));
    assert_eq!(frame.pixels[0], 7);
    assert_eq!(*frame.pixels.last().unwrap(), 9);
    assert!(frame.pixels.iter().all(|p| *p == 7 || *p == 9));
    FrmArchive::parse(&LocalWriters.frm(&spec).unwrap()).unwrap();
    // The transparent index is refused from the palette mapping.
    let zero = |_: [u8; 3]| 0;
    assert!(matches!(
        inventory_frm_rgba(
            &rgba,
            width as u32,
            height as u32,
            INVENTORY_ICON_MAX,
            &zero
        ),
        Err(EmitError::Image(_))
    ));
    assert!(
        inventory_frm_rgba(&rgba[4..], width as u32, height as u32, [90, 61], &quantize).is_err()
    );
}

#[test]
fn item_art_goes_in_both_lists_and_resolves() {
    let mut items = ArtList::from_base(ITEM_CATEGORY, b"base.frm\n").unwrap();
    let mut inven = ArtList::from_base(INVENTORY_CATEGORY, b"ibase.frm\n").unwrap();
    let art = push_item_art(&mut items, &mut inven, "mfpis01.frm", "mfipis01.frm").unwrap();
    assert_eq!(art.ground.category(), 0);
    assert_eq!(art.inventory.category(), 7);
    assert_eq!((art.ground.base(), art.inventory.base()), (1, 1));
    assert_eq!(items.path(art.ground).unwrap(), "art/items/mfpis01.frm");
    assert_eq!(inven.path(art.inventory).unwrap(), "art/inven/mfipis01.frm");
    // Pushing again finds the same entries.
    let again = push_item_art(&mut items, &mut inven, "MFPIS01.FRM", "mfipis01.frm").unwrap();
    assert_eq!(again, art);
    assert_eq!((items.len(), inven.len()), (2, 2));
    // Lists swapped are refused.
    assert!(push_item_art(&mut inven, &mut items, "a.frm", "b.frm").is_err());
}
