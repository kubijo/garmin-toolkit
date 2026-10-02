SELECT pragma_page_count.page_count * pragma_page_size.page_size AS bytes
FROM pragma_page_count, pragma_page_size;
