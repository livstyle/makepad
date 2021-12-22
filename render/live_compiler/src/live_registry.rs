//use crate::id::Id;
use {
    std::collections::{HashMap, BTreeSet},
    makepad_id_macros::*,
    makepad_live_tokenizer::{Delim, TokenPos, TokenRange, TokenWithLen, FullToken, LiveId, State, Cursor},
    crate::{
        live_error::{LiveError, LiveErrorOrigin, LiveFileError},
        live_parser::LiveParser,
        live_document::LiveDocument,
        live_node::{LiveNode, LiveValue, LiveType, LiveTypeInfo, LiveNodeOrigin},
        live_node_vec::{LiveNodeSlice, LiveNodeVec, LiveNodeMutReader},
        live_ptr::{LiveFileId, LivePtr, LiveModuleId},
        live_token::{LiveToken, LiveTokenId, TokenWithSpan},
        span::{Span, TextPos},
        live_expander::{LiveExpander}
    }
};

#[derive(Default)]
pub struct LiveFile {
    pub module_id: LiveModuleId,
    pub start_pos: TextPos,
    pub file_name: String,
    pub source: String,
    pub deps: BTreeSet<LiveModuleId>,
    pub document: LiveDocument,
}

pub struct LiveRegistry {
    pub file_ids: HashMap<String, LiveFileId>,
    pub module_id_to_file_id: HashMap<LiveModuleId, LiveFileId>,
    pub live_files: Vec<LiveFile>,
    pub live_type_infos: HashMap<LiveType, LiveTypeInfo>,
    pub expanded: Vec<LiveDocument>,
    pub main_module: Option<LiveFileId>,
    pub main_apply: Option<Vec<LiveNode >>
}

impl Default for LiveRegistry {
    fn default() -> Self {
        Self {
            main_module: None,
            file_ids: HashMap::new(),
            module_id_to_file_id: HashMap::new(),
            live_files: vec![LiveFile::default()],
            live_type_infos: HashMap::new(),
            expanded: vec![LiveDocument::default()],
            main_apply: None
        }
    }
}

pub struct LiveDocNodes<'a> {
    pub nodes: &'a [LiveNode],
    pub file_id: LiveFileId,
    pub index: usize
}

#[derive(Copy, Clone, Debug)]
pub enum LiveScopeTarget {
    LocalPtr(usize),
    LivePtr(LivePtr)
}

impl LiveRegistry {
    pub fn ptr_to_node(&self, live_ptr: LivePtr) -> &LiveNode {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        &doc.resolve_ptr(live_ptr.index as usize)
    }
    
    pub fn file_id_to_file_name(&self, file_id: LiveFileId) -> &str {
        &self.live_files[file_id.to_index()].file_name
    }
    
    pub fn ptr_to_doc_node(&self, live_ptr: LivePtr) -> (&LiveDocument, &LiveNode) {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        (doc, &doc.resolve_ptr(live_ptr.index as usize))
    }
    
    pub fn ptr_to_doc(&self, live_ptr: LivePtr) -> &LiveDocument {
        &self.expanded[live_ptr.file_id.to_index()]
    }
    
    pub fn file_id_to_doc(&self, file_id: LiveFileId) -> &LiveDocument {
        &self.expanded[file_id.to_index()]
    }
    
    pub fn ptr_to_nodes_index(&self, live_ptr: LivePtr) -> (&[LiveNode], usize) {
        let doc = &self.expanded[live_ptr.file_id.to_index()];
        (&doc.nodes, live_ptr.index as usize)
    }
    
    pub fn path_str_to_file_id(&self, path: &str) -> Option<LiveFileId> {
        for (index, file) in self.live_files.iter().enumerate() {
            if file.file_name == path {
                return Some(LiveFileId(index as u16))
            }
        }
        None
    }
    
    pub fn token_id_to_origin_doc(&self, token_id: LiveTokenId) -> &LiveDocument {
        &self.live_files[token_id.file_id().to_index()].document
    }
    
    pub fn token_id_to_expanded_doc(&self, token_id: LiveTokenId) -> &LiveDocument {
        &self.expanded[token_id.file_id().to_index()]
    }
    
    pub fn module_id_to_file_id(&self, module_id: LiveModuleId) -> Option<LiveFileId> {
        self.module_id_to_file_id.get(&module_id).cloned()
    }
    
    pub fn module_id_and_name_to_doc(&self, module_id: LiveModuleId, name: LiveId) -> Option<LiveDocNodes> {
        if let Some(file_id) = self.module_id_to_file_id.get(&module_id) {
            let doc = &self.expanded[file_id.to_index()];
            if name != LiveId::empty() {
                if doc.nodes.len() == 0 {
                    println!("module_path_id_to_doc zero nodelen {}", self.file_id_to_file_name(*file_id));
                    return None
                }
                if let Some(index) = doc.nodes.child_by_name(0, name) {
                    return Some(LiveDocNodes {nodes: &doc.nodes, file_id: *file_id, index});
                }
                else {
                    return None
                }
            }
            else {
                return Some(LiveDocNodes {nodes: &doc.nodes, file_id: *file_id, index: 0});
            }
        }
        None
    }
    
    pub fn find_scope_item_via_class_parent(&self, start_ptr: LivePtr, item: LiveId) -> Option<(&[LiveNode], usize)> {
        let (nodes, index) = self.ptr_to_nodes_index(start_ptr);
        if let LiveValue::Class {class_parent, ..} = &nodes[index].value {
            // ok its a class so now first scan up from here.
            
            if let Some(index) = nodes.scope_up_down_by_name(index, item) {
                // item can be a 'use' as well.
                // if its a use we need to resolve it, otherwise w found it
                if let LiveValue::Use(module_id) = &nodes[index].value {
                    if let Some(ldn) = self.module_id_and_name_to_doc(*module_id, nodes[index].id) {
                        return Some((ldn.nodes, ldn.index))
                    }
                }
                else {
                    return Some((nodes, index))
                }
            }
            else {
                if let Some(class_parent) = class_parent {
                    if class_parent.file_id != start_ptr.file_id {
                        return self.find_scope_item_via_class_parent(*class_parent, item)
                    }
                }
                
            }
        }
        else {
            println!("WRONG TYPE  {:?}", nodes[index].value);
        }
        None
    }
    
    pub fn find_scope_target_via_start(&self, item: LiveId, index: usize, nodes: &[LiveNode]) -> Option<LiveScopeTarget> {
        if let Some(index) = nodes.scope_up_down_by_name(index, item) {
            if let LiveValue::Use(module_id) = &nodes[index].value {
                // ok lets find it in that other doc
                if let Some(file_id) = self.module_id_to_file_id(*module_id) {
                    let doc = self.file_id_to_doc(file_id);
                    if let Some(index) = doc.nodes.child_by_name(0, item) {
                        return Some(LiveScopeTarget::LivePtr(
                            LivePtr {file_id: file_id, index: index as u32}
                        ))
                    }
                }
            }
            else {
                return Some(LiveScopeTarget::LocalPtr(index))
            }
        }
        // ok now look at the glob use * things
        let mut node_iter = Some(1);
        while let Some(index) = node_iter {
            if let LiveValue::Use(module_id) = &nodes[index].value {
                if nodes[index].id == LiveId::empty() { // glob
                    if let Some(file_id) = self.module_id_to_file_id(*module_id) {
                        let doc = self.file_id_to_doc(file_id);
                        if let Some(index) = doc.nodes.child_by_name(0, item) {
                            return Some(LiveScopeTarget::LivePtr(
                                LivePtr {file_id: file_id, index: index as u32}
                            ))
                        }
                    }
                }
            }
            node_iter = nodes.next_child(index);
        }
        None
    }
    
    pub fn find_scope_ptr_via_origin(&self, origin: LiveNodeOrigin, item: LiveId) -> Option<LivePtr> {
        // ok lets start
        let token_id = origin.token_id().unwrap();
        let index = origin.node_index().unwrap();
        let file_id = token_id.file_id();
        let doc = self.file_id_to_doc(file_id);
        match self.find_scope_target_via_start(item, index, &doc.nodes) {
            Some(LiveScopeTarget::LocalPtr(index)) => Some(LivePtr {file_id: file_id, index: index as u32}),
            Some(LiveScopeTarget::LivePtr(ptr)) => Some(ptr),
            None => None
        }
    }
    
    pub fn live_error_to_live_file_error(&self, live_error: LiveError) -> LiveFileError {
        let live_file = &self.live_files[live_error.span.file_id.to_index()];
        live_error.to_live_file_error(&live_file.file_name)
    }
    
    pub fn token_id_to_span(&self, token_id: LiveTokenId) -> Span {
        self.live_files[token_id.file_id().to_index()].document.token_id_to_span(token_id)
    }
    /*
    pub fn insert_dep_order(&mut self, module_id: LiveModuleId, token_id: TokenId, own_module_id: LiveModuleId) {
        let self_index = self.dep_order.iter().position( | v | v.0 == own_module_id).unwrap();
        if let Some(other_index) = self.dep_order.iter().position( | v | v.0 == module_id) {
            // if other_index is > self index. we should move self later
            
            if other_index > self_index {
                self.dep_order.remove(other_index);
                self.dep_order.insert(self_index, (module_id, token_id));
            }
        } 
        else {
            self.dep_order.insert(self_index, (module_id, token_id));
        }
    }*/
    
    pub fn tokenize_from_str(source: &str, start_pos: TextPos, file_id: LiveFileId) -> Result<(Vec<TokenWithSpan>, Vec<char>), LiveError> {
        let mut line_chars = Vec::new();
        let mut state = State::default();
        let mut scratch = String::new();
        let mut strings = Vec::new();
        let mut tokens = Vec::new();
        let mut pos = start_pos;
        for line_str in source.lines() {
            line_chars.truncate(0);
            line_chars.extend(line_str.chars());
            let mut cursor = Cursor::new(&line_chars, &mut scratch);
            loop {
                let (next_state, full_token) = state.next(&mut cursor);
                if let Some(full_token) = full_token {
                    let span = Span {
                        file_id,
                        start: pos,
                        end: TextPos {column: pos.column + full_token.len as u32, line: pos.line}
                    };
                    match full_token.token {
                        FullToken::Unknown | FullToken::OtherNumber | FullToken::Lifetime => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: span,
                                message: format!("Error tokenizing")
                            })
                        },
                        FullToken::String => {
                            let len = full_token.len - 2;
                            tokens.push(TokenWithSpan {span: span, token: LiveToken::String {
                                index: strings.len() as u32,
                                len: len as u32
                            }});
                            let col = pos.column as usize + 1;
                            strings.extend(&line_chars[col..col + len]);
                        },
                        _ => match LiveToken::from_full_token(full_token.token) {
                            Some(live_token) => {
                                // lets build up the span info
                                tokens.push(TokenWithSpan {span: span, token: live_token})
                            },
                            _ => ()
                        },
                    }
                    pos.column += full_token.len as u32;
                }
                else {
                    break;
                }
                state = next_state;
            }
            pos.line += 1;
            pos.column = 0;
        }
        tokens.push(TokenWithSpan {span: Span::default(), token: LiveToken::Eof});
        Ok((tokens, strings))
    }
    
    // called by the live editor to update a live file
    pub fn update_live_file<'a, CB>(
        &mut self,
        file_name: &str,
        range: TokenRange,
        mut get_line: CB
    ) -> Result<bool, LiveError>
    where CB: FnMut(usize) -> (&'a [char], &'a [TokenWithLen])
    {
        let file_id = *self.file_ids.get(file_name).unwrap();
        let mut live_index = 0;
        let document = &mut self.live_files[file_id.to_index()].document;
        
        let live_tokens = &mut document.tokens;
        let old_strings = &document.strings;
        let mut new_strings: Vec<char> = Vec::new();
        
        let mut mutations = Vec::new();
        let mut parse_changed = false;
        
        for line in range.start.line..range.end.line {
            let (line_chars, full_tokens) = get_line(line);
            // OK SO now we diff as we go
            let mut column = 0usize;
            for (token_index, full_token) in full_tokens.iter().enumerate() {
                
                if range.is_in_range(TokenPos {line: line, index: token_index}) {
                    // ok so. now we filter the token
                    let span = Span {
                        file_id,
                        start: TextPos {column: column as u32, line: line as u32},
                        end: TextPos {column: (column + full_token.len) as u32, line: line as u32}
                    };
                    
                    match full_token.token {
                        FullToken::Unknown | FullToken::OtherNumber | FullToken::Lifetime => {
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: span,
                                message: format!("Error tokenizing")
                            })
                        },
                        FullToken::String => {
                            let new_len = full_token.len - 2;
                            let new_col = column as usize + 1;
                            let new_chars = &line_chars[new_col..new_col + new_len];
                            let new_string = LiveToken::String {
                                index: new_strings.len() as u32,
                                len: new_len as u32
                            };
                            if live_index >= live_tokens.len() { // just append
                                parse_changed = true;
                                live_tokens.push(TokenWithSpan {span: span, token: new_string});
                            }
                            else if let LiveToken::String {index, len} = live_tokens[live_index].token {
                                let old_chars = &old_strings[index as usize ..(index + len) as usize];
                                // compare string or len/position
                                if new_chars != old_chars || new_strings.len() as u32 != index || new_len as u32 != len {
                                    mutations.push(live_index);
                                }
                            }
                            else { // cant replace a sttring type with something else without a reparse
                                parse_changed = true;
                                live_tokens[live_index] = TokenWithSpan {span: span, token: new_string};
                            }
                            new_strings.extend(new_chars);
                            live_index += 1;
                        },
                        _ => match LiveToken::from_full_token(full_token.token) {
                            Some(live_token) => {
                                if live_index >= live_tokens.len() { // just append
                                    parse_changed = true;
                                    live_tokens.push(TokenWithSpan {span: span, token: live_token})
                                }
                                else {
                                    if live_tokens[live_index].is_parse_equal(live_token) { // token value changed
                                        if live_tokens[live_index].token != live_token {
                                            live_tokens[live_index].token = live_token;
                                            mutations.push(live_index);
                                        }
                                    }
                                    else { // token value changed in a way that changes parsing
                                        // lets special case the {{id}} situation
                                        if live_index > 2
                                            && live_tokens[live_index - 2].is_open_delim(Delim::Brace)
                                            && live_tokens[live_index - 1].is_open_delim(Delim::Brace)
                                            && live_tokens[live_index].is_int()
                                            && live_token.is_ident() {
                                            // ignore it.
                                        }
                                        else {
                                            parse_changed = true;
                                            live_tokens[live_index].token = live_token;
                                        }
                                    }
                                }
                                // always update the spans
                                live_tokens[live_index].span = span;
                                live_index += 1;
                            },
                            _ => ()
                        },
                    }
                }
                column += full_token.len;
            }
        }
        if live_index < live_tokens.len() - 1 {
            parse_changed = true;
        }
        
        if parse_changed {
            println!("WE HAVE TO REPARSE {:?}", range);
            return Ok(true)
        }
        else if mutations.len()>0 { // we got mutations
            document.strings = new_strings;
            self.apply_mutations(file_id, &mutations);
            return Ok(true)
        }
        
        Ok(false)
    }
    
    fn apply_mutations(
        &mut self,
        file_id: LiveFileId,
        mutations: &[usize],
    ) {
        let mut main_apply = Vec::new();
        main_apply.open();
        
        for mutation in mutations {
            // ok so. lets see if we have a prop:value change
            let document = &self.live_files[file_id.to_index()].document;
            let live_tokens = &document.tokens;
            
            let is_prop_assign = *mutation > 2
                && live_tokens[mutation - 2].is_ident()
                && live_tokens[mutation - 1].is_punct_id(id!(:));
            
            if is_prop_assign || live_tokens[*mutation].is_value_type() {
                let token_id = LiveTokenId::new(file_id, mutation - 2);
                
                // ok lets scan for this one.
                let mut file_dep_iter = FileDepIter::new(file_id);
                let mut path = Vec::new();
                while let Some(file_id) = file_dep_iter.pop_todo() {
                    let expanded = &mut self.expanded[file_id.to_index()];
                    // TODO add in-DSL token scans
                    let mut reader = LiveNodeMutReader::new(0, &mut expanded.nodes);
                    path.truncate(0);
                    reader.walk();
                    while !reader.is_eot() {
                        if reader.is_open() {
                            path.push(reader.id)
                        }
                        else if reader.is_close() {
                            path.pop();
                        }
                        // ok this is a direct patch
                        else if is_prop_assign && reader.origin.token_id() == Some(token_id){
                                
                            if !reader.update_from_live_token(&live_tokens[*mutation].token) {
                                println!("update_from_live_token returns false investigate! {:?}", reader.node());
                            }
                            if self.main_module == Some(file_id) {
                                // ok so. lets write by path here
                                path.push(reader.id);
                                main_apply.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                                path.pop();
                            }
                        }
                        else if reader.is_token_id_inside_dsl(token_id){
                            if self.main_module == Some(file_id) {
                                // ok so. lets write by path here
                                path.push(reader.id);
                                main_apply.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                                path.pop();
                            }
                        }
                        reader.walk();
                    }
                    
                    file_dep_iter.scan_next(&self.live_files);
                }
            }
        }
        main_apply.close();
        //println!("{}", main_apply.to_string(0,100));
        self.main_apply = Some(main_apply);
        
    }
    
    pub fn register_live_file(
        &mut self,
        file_name: &str,
        own_module_id: LiveModuleId,
        source: String,
        live_type_infos: Vec<LiveTypeInfo>,
        start_pos: TextPos,
    ) -> Result<LiveFileId, LiveFileError> {
        
        // lets register our live_type_infos
        if self.file_ids.get(file_name).is_some() {
            panic!("cant register same file twice");
        }
        let file_id = LiveFileId::new(self.live_files.len());
        
        let (tokens, strings) = match Self::tokenize_from_str(&source, start_pos, file_id) {
            Err(msg) => return Err(msg.to_live_file_error(file_name)), //panic!("Lex error {}", msg),
            Ok(lex_result) => lex_result
        };
        
        let mut parser = LiveParser::new(&tokens, &live_type_infos, file_id);
        
        let mut document = match parser.parse_live_document() {
            Err(msg) => return Err(msg.to_live_file_error(file_name)), //panic!("Parse error {}", msg.to_live_file_error(file, &source)),
            Ok(ld) => ld
        };
        
        document.strings = strings;
        document.tokens = tokens;
        
        // update our live type info
        for live_type_info in live_type_infos {
            if let Some(info) = self.live_type_infos.get(&live_type_info.live_type) {
                if info.module_id != live_type_info.module_id ||
                info.live_type != live_type_info.live_type {
                    panic!()
                }
            };
            self.live_type_infos.insert(live_type_info.live_type, live_type_info);
        }
        
        let mut deps = BTreeSet::new();
        
        for node in &mut document.nodes {
            match &mut node.value {
                LiveValue::Use(module_id) => {
                    if module_id.0 == id!(crate) { // patch up crate refs
                        module_id.0 = own_module_id.0
                    };
                    deps.insert(*module_id);
                }, // import
                LiveValue::Class {live_type, ..} => { // hold up. this is always own_module_path
                    let infos = self.live_type_infos.get(&live_type).unwrap();
                    for sub_type in infos.fields.clone() {
                        let sub_module_id = sub_type.live_type_info.module_id;
                        if sub_module_id != own_module_id {
                            deps.insert(sub_module_id);
                        }
                    }
                }
                _ => {
                }
            }
        }
        
        let live_file = LiveFile {
            module_id: own_module_id,
            file_name: file_name.to_string(),
            start_pos,
            deps,
            source,
            document
        };
        self.module_id_to_file_id.insert(own_module_id, file_id);
        
        self.file_ids.insert(file_name.to_string(), file_id);
        self.live_files.push(live_file);
        self.expanded.push(LiveDocument::new());
        
        return Ok(file_id)
    }
    
    /*
    pub fn update_live_file(
        &mut self,
        file_name: &str,
        file_id: LiveFileId,
        source: String,
        live_type_infos: Vec<LiveTypeInfo>,
        start_pos: TextPos,
    ) -> Result<(), LiveFileError> {
        Ok(())
    }*/
    
    pub fn expand_all_documents(&mut self, errors: &mut Vec<LiveError>) {
        
        // alright so. we iterate
        let mut dep_order = Vec::new();
        
        fn recur_insert_dep(parent_index: usize, dep_order: &mut Vec<LiveModuleId>, current: LiveModuleId, files: &Vec<LiveFile>) {
            let file = if let Some(file) = files.iter().find( | v | v.module_id == current) {
                file
            }
            else {
                return
            };
            let final_index = if let Some(index) = dep_order.iter().position( | v | *v == current) {
                if index > parent_index { // insert before
                    dep_order.remove(index);
                    dep_order.insert(parent_index, current);
                    parent_index
                }
                else {
                    index
                }
            }
            else {
                let index = dep_order.len();
                dep_order.push(current);
                index
            };
            
            for dep in &file.deps {
                recur_insert_dep(final_index, dep_order, *dep, files);
            }
        }
        
        for file in &self.live_files {
            recur_insert_dep(dep_order.len(), &mut dep_order, file.module_id, &self.live_files);
        }
        
        //self.top_level_file = self.module_id_to_file_id.get(dep_order.last().unwrap()).cloned();
        
        for crate_module in dep_order {
            let file_id = if let Some(file_id) = self.module_id_to_file_id.get(&crate_module) {
                file_id
            }
            else {
                continue
            };
            
            if !self.expanded[file_id.to_index()].recompile {
                continue;
            }
            let live_file = &self.live_files[file_id.to_index()];
            let in_doc = &live_file.document;
            
            let mut out_doc = LiveDocument::new();
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
            out_doc.restart_from(&in_doc);
            
            
            let mut live_document_expander = LiveExpander {
                live_registry: self,
                in_crate: crate_module.0,
                in_file_id: *file_id,
                errors
            };
            live_document_expander.expand(in_doc, &mut out_doc);
            
            out_doc.recompile = false;
            
            std::mem::swap(&mut out_doc, &mut self.expanded[file_id.to_index()]);
        }
    }
}

struct FileDepIter {
    files_todo: Vec<LiveFileId>,
    files_done: Vec<LiveFileId>
}

impl FileDepIter {
    pub fn new(start: LiveFileId) -> Self {
        Self {
            files_todo: vec![start],
            files_done: Vec::new()
        }
    }
    
    pub fn pop_todo(&mut self) -> Option<LiveFileId> {
        if let Some(file_id) = self.files_todo.pop() {
            self.files_done.push(file_id);
            Some(file_id)
        }
        else {
            None
        }
    }
    
    pub fn scan_next(&mut self, live_files: &[LiveFile]) {
        let last_file_id = self.files_done.last().unwrap();
        let module_id = live_files[last_file_id.to_index()].module_id;
        
        for (file_index, live_file) in live_files.iter().enumerate() {
            if live_file.deps.contains(&module_id) {
                let dep_id = LiveFileId::new(file_index);
                if self.files_done.iter().position( | v | *v == dep_id).is_none() {
                    self.files_todo.push(dep_id);
                }
            }
        }
    }
}
