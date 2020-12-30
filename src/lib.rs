use std::collections::BTreeMap;

use lopdf::{dictionary, Document, Object, ObjectId};
use std::path::Path;

/// Get the number of pages from a pdf.
///
/// # Panic
/// This function will panic if the pdf does not exist or otherwise cannot be opened.
pub fn pdf_get_size(pdf: &Path) -> usize {
    let document = Document::load(pdf);
    let document = document.unwrap();
    document.page_iter().count()
}

/// Make sure that the pdf has a even number of pages. This may be desirable if a pdf is merged that
/// should be double-sided printed.
///
/// # Panic
/// This function will panic if the pdf does not exist or otherwise cannot be opened.
/// This function will panic if the pdf has an uexpectred structure
pub fn make_page_count_even(pdf: &Path) {
    let mut document = Document::load(pdf).unwrap();
    let document_length = document.get_pages().len() as u32;

    if document_length % 2 != 0 {
        let catalog = document.catalog().unwrap();
        let pages_id_ref = catalog.get(b"Pages").unwrap();
        let (pages_id, _) = document.dereference(pages_id_ref).unwrap();
        let pages_id = pages_id.unwrap();

        // Create and add a new empty page
        let page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
        };
        let page_id = document.add_object(page);

        // add the new page to the pages and update the page count
        let pages = document.get_object_mut(pages_id).unwrap();
        let pages = pages.as_dict_mut().unwrap();

        let pages_kids = pages.get_mut(b"Kids").unwrap();
        let pages_kids = pages_kids.as_array_mut()
            .unwrap();
        pages_kids.push(Object::Reference(page_id));

        let pages_count = pages.get_mut(b"Count").unwrap();
        let new_count = pages_count.as_i64().unwrap() + 1;
        pages.set("Count", new_count as i64);

        document.renumber_objects();
        document.save(pdf).unwrap();
    }
}

/// Insert a pdf `source` in possible multiple places in another pdf `destination`.
///
/// # NOTE:
/// This overwrites the pdf `destination`.
///
/// # Panic
/// This panics, if
/// - src is not a pdf or cannot be opened
/// - dst is not a pdf or cannot be opened
/// - the structure of the pdf is not as expected.
///
pub fn insert(destination: &Path, after_pages: &Vec<u32>, source: &Path) {
    let mut document_dst = Document::load(destination).unwrap();
    let mut document_src = Document::load(source).unwrap();

    // Initialize a new empty document
    let mut document = Document::with_version("1.5");

    // Define a starting max_id (will be used as start index for object_ids)
    let mut max_id;

    // Collect all Documents Objects grouped by a map
    let mut documents_pages = BTreeMap::new();
    let mut documents_objects = BTreeMap::new();

    // Get number of pages
    let dst_num_pages = document_dst.get_pages().len() as u32;
    let src_num_pages = document_src.get_pages().len() as u32;
    let num_pages = dst_num_pages + after_pages.len() as u32 * src_num_pages;
    println!("Will result in {} pages", num_pages);

    max_id = num_pages + 1;
    //println!("max id {}", max_id);

    document_dst.renumber_objects_with(max_id);

    max_id = document_dst.max_id;

    let mut origin_page_count: u32 = 0;
    let mut result_page_index: u32 = 0;

    // Add the pages from dst with the correct indexes
    documents_pages.extend(
        document_dst
            .get_pages()
            .into_iter()
            .map(|(_, object_id)| {
                if after_pages.contains(&origin_page_count) {
                    result_page_index += src_num_pages;
                }
                result_page_index += 1;
                origin_page_count += 1;
                (
                    (result_page_index, object_id.1),
                    document_dst.get_object(object_id).unwrap().to_owned(),
                )
            })
            .collect::<BTreeMap<ObjectId, Object>>(),
    );
    documents_objects.extend(document_dst.objects);

    assert_eq!(dst_num_pages, origin_page_count);
    assert_eq!(documents_pages.len() as u32, dst_num_pages);

    // renumber the objects to make sure that the indexes don't collide with the indexes from the other file.
    document_src.renumber_objects_with(max_id);

    // Add the pages from src with the correct indexes
    let mut added_pages = 0;
    for i in after_pages {
        result_page_index = i + added_pages;
        documents_pages.extend(
            document_src
                .get_pages()
                .into_iter()
                .map(|(_, object_id)| {
                    result_page_index += 1;
                    (
                        (result_page_index, object_id.1),
                        document_src.get_object(object_id).unwrap().to_owned(),
                    )
                })
                .collect::<BTreeMap<ObjectId, Object>>(),
        );
        added_pages += src_num_pages;
    }
    documents_objects.extend(document_src.objects);

    assert_eq!(documents_pages.len() as u32, num_pages);

    // Catalog and Pages are mandatory
    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    // Process all objects except "Page" type
    for (object_id, object) in documents_objects.iter() {
        // We have to ignore "Page", "Outlines" and "Outline" objects
        // All other objects should be collected and inserted into the main Document
        match object.type_name().unwrap_or("") {
            "Catalog" => {
                // Collect a first "Catalog" object and use it for the future "Pages"
                catalog_object = Some((
                    if let Some((id, _)) = catalog_object {
                        id
                    } else {
                        *object_id
                    },
                    object.clone(),
                ));
            }
            "Pages" => {
                // Collect and update a first "Pages" object and use it for the future "Catalog"
                // We have also to merge all dictionaries of the old and the new "Pages" object
                if let Ok(dictionary) = object.as_dict() {
                    let mut dictionary = dictionary.clone();
                    if let Some((_, ref object)) = pages_object {
                        if let Ok(old_dictionary) = object.as_dict() {
                            dictionary.extend(old_dictionary);
                        }
                    }

                    pages_object = Some((
                        if let Some((id, _)) = pages_object {
                            id
                        } else {
                            *object_id
                        },
                        Object::Dictionary(dictionary),
                    ));
                }
            }
            "Page" => {}     // Ignored, processed later and separately
            "Outlines" => {
                println!("Outlines not suppoted");
            }
            "Outline" => {
                println!("Outline not suppoted");
            }
            _ => {
                document.objects.insert(*object_id, object.clone());
            }
        }
    }

    // If no "Pages" found abort
    if pages_object.is_none() {
        println!("Pages root not found.");
        return;
    }

    // Iter over all "Page" and collect with the parent "Pages" created before
    for (object_id, object) in documents_pages.iter() {
        if let Ok(dictionary) = object.as_dict() {
            let mut dictionary = dictionary.clone();
            dictionary.set("Parent", pages_object.as_ref().unwrap().0);

            document
                .objects
                .insert(*object_id, Object::Dictionary(dictionary));
        }
    }

    // If no "Catalog" found abort
    if catalog_object.is_none() {
        println!("Catalog root not found.");
        return;
    }

    let catalog_object = catalog_object.unwrap();
    let pages_object = pages_object.unwrap();

    // Build a new "Pages" with updated fields
    if let Ok(dictionary) = pages_object.1.as_dict() {
        let mut dictionary = dictionary.clone();

        // Set new pages count
        dictionary.set("Count", documents_pages.len() as u32);

        // Set new "Kids" list (collected from documents pages) for "Pages"
        dictionary.set(
            "Kids",
            documents_pages
                .into_iter()
                .map(|(object_id, _)| Object::Reference(object_id))
                .collect::<Vec<_>>(),
        );

        document
            .objects
            .insert(pages_object.0, Object::Dictionary(dictionary));
    }

    // Build a new "Catalog" with updated fields
    if let Ok(dictionary) = catalog_object.1.as_dict() {
        let mut dictionary = dictionary.clone();
        dictionary.set("Pages", pages_object.0);
        dictionary.remove(b"Outlines"); // Outlines not supported in merged PDFs

        document
            .objects
            .insert(catalog_object.0, Object::Dictionary(dictionary));
    }

    document.trailer.set("Root", catalog_object.0);

    // Update the max internal ID as wasn't updated before due to direct objects insertion
    document.max_id = document.objects.len() as u32;

    // Reorder all new Document objects
    document.renumber_objects();
    document.compress();

    // Save the merged PDF
    document.save(destination).unwrap();
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    #[test]
    fn merge() -> Result<(), std::io::Error> {
        fs::copy("./Test1.pdf", "./Test1_tmp.pdf")?;
        let a = Path::new("./Test1_tmp.pdf");
        let b = Path::new("./Test2.pdf");

        let length_a = super::pdf_get_size(a);
        assert_eq!(length_a, 9);
        let length_b = super::pdf_get_size(b);
        assert_eq!(length_b, 2);

        let target_indexes: Vec<u32> = vec![0, 1, 2, 4, length_a as u32 - 1];
        super::insert(&a, &target_indexes, &b);
        assert_eq!(
            super::pdf_get_size(a),
            length_a + target_indexes.len() * length_b
        );

        fs::remove_file(a)?;
        return Ok(());
    }

    #[test]
    fn make_even() -> Result<(), std::io::Error> {
        fs::copy("./Test1.pdf", "./Test1_tmp2.pdf")?;
        let a = Path::new("./Test1_tmp2.pdf");

        let size = super::pdf_get_size(a);
        assert_eq!(size % 2, 1);
        super::make_page_count_even(a);
        assert_eq!(super::pdf_get_size(a) % 2, 0);
        assert_eq!(super::pdf_get_size(a), size + 1);

        fs::remove_file(a)?;
        return Ok(());
    }
}
