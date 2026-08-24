use rusqlite::params;
use serde::Serialize;
use std::{collections::HashSet, path::Path, sync::{Mutex, OnceLock}};
use tauri::AppHandle;
use windows::{
    core::HSTRING,
    Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions},
    Foundation::{Rect, Size},
    Graphics::Imaging::BitmapDecoder,
    Media::Ocr::OcrEngine,
    Storage::StorageFile,
    Storage::Streams::InMemoryRandomAccessStream,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

const FOCUSED_LONG_EDGE: f32 = 3000.0;
const MAX_FOCUSED_PAGES: usize = 4;
static FOCUSED_ATTEMPTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Default, Serialize)]
pub struct FocusedOptimizationResult {
    pub inspected: usize,
    pub processed: usize,
    pub improved: usize,
    pub errors: usize,
}

struct WinRtGuard;
impl WinRtGuard {
    fn initialize() -> Result<Self, String> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED).map_err(|e| format!("Initialisation OCR focalisé impossible : {e}"))?; }
        Ok(Self)
    }
}
impl Drop for WinRtGuard { fn drop(&mut self) { unsafe { RoUninitialize() }; } }

fn render_dimensions(width:f32,height:f32,max_dimension:u32,requested:f32)->(u32,u32){
    let width=width.max(1.0); let height=height.max(1.0); let target=requested.min(max_dimension as f32).max(1.0); let scale=target/width.max(height);
    ((width*scale).round().max(1.0) as u32,(height*scale).round().max(1.0) as u32)
}
fn line_key(v:&str)->String{v.chars().flat_map(|c|c.to_lowercase()).filter(|c|c.is_alphanumeric()).collect()}
fn merge_lines(target:&mut String,seen:&mut HashSet<String>,text:&str){for line in text.lines(){let t=line.trim();if t.is_empty(){continue}let k=line_key(t);if k.len()<2||!seen.insert(k){continue}if !target.is_empty(){target.push('\n')}target.push_str(t)}}
fn merge_texts(first:&str,second:&str)->String{let mut out=String::new();let mut seen=HashSet::new();merge_lines(&mut out,&mut seen,first);merge_lines(&mut out,&mut seen,second);out}
fn bounded_rect(x:f32,y:f32,width:f32,height:f32,size:Size)->Rect{let pw=size.Width.max(1.0);let ph=size.Height.max(1.0);let x=x.clamp(0.0,pw-1.0);let y=y.clamp(0.0,ph-1.0);Rect{X:x,Y:y,Width:width.min(pw-x).max(1.0),Height:height.min(ph-y).max(1.0)}}
fn receipt_regions(size:Size)->Vec<Rect>{let w=size.Width.max(1.0);let h=size.Height.max(1.0);let bh=h*0.28;[0.0_f32,0.15,0.30,0.45,0.60,0.72].into_iter().map(|s|bounded_rect(0.0,h*s,w,bh,size)).collect()}
fn invoice_regions(size:Size)->Vec<Rect>{let w=size.Width.max(1.0);let h=size.Height.max(1.0);vec![bounded_rect(0.0,0.0,w*0.60,h*0.42,size),bounded_rect(w*0.40,0.0,w*0.60,h*0.42,size),bounded_rect(0.0,h*0.24,w,h*0.48,size),bounded_rect(0.0,h*0.56,w*0.60,h*0.44,size),bounded_rect(w*0.40,h*0.56,w*0.60,h*0.44,size)]}
fn selected_page_indexes(page_count:u32)->Vec<u32>{if page_count as usize<=MAX_FOCUSED_PAGES{return(0..page_count).collect()}let mut v=vec![0,1,page_count-2,page_count-1];v.sort_unstable();v.dedup();v}
fn ocr_region(page:&PdfPage,engine:&OcrEngine,max_dimension:u32,region:Rect)->Result<String,String>{
    let (dw,dh)=render_dimensions(region.Width,region.Height,max_dimension,FOCUSED_LONG_EDGE);let options=PdfPageRenderOptions::new().map_err(|e|e.to_string())?;options.SetSourceRect(region).map_err(|e|e.to_string())?;options.SetDestinationWidth(dw).map_err(|e|e.to_string())?;options.SetDestinationHeight(dh).map_err(|e|e.to_string())?;
    let stream=InMemoryRandomAccessStream::new().map_err(|e|e.to_string())?;page.RenderWithOptionsToStreamAsync(&stream,&options).map_err(|e|e.to_string())?.join().map_err(|e|e.to_string())?;stream.Seek(0).map_err(|e|e.to_string())?;
    let decoder=BitmapDecoder::CreateAsync(&stream).map_err(|e|e.to_string())?.join().map_err(|e|e.to_string())?;let bitmap=decoder.GetSoftwareBitmapAsync().map_err(|e|e.to_string())?.join().map_err(|e|e.to_string())?;let result=engine.RecognizeAsync(&bitmap).map_err(|e|e.to_string())?.join().map_err(|e|e.to_string())?;result.Text().map_err(|e|e.to_string()).map(|v|v.to_string_lossy())
}

pub fn ocr_focused_pdf(source:&str,receipt_hint:bool)->Result<String,String>{
    let path=Path::new(source);if !path.is_file(){return Err("Le PDF n'est plus accessible pour la lecture focalisée.".to_string())}
    let _winrt=WinRtGuard::initialize()?;let file=StorageFile::GetFileFromPathAsync(&HSTRING::from(path.to_string_lossy().into_owned())).map_err(|e|e.to_string())?.join().map_err(|e|e.to_string())?;let document=PdfDocument::LoadFromFileAsync(&file).map_err(|e|e.to_string())?.join().map_err(|e|e.to_string())?;let engine=OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e|e.to_string())?;let max_dimension=OcrEngine::MaxImageDimension().map_err(|e|e.to_string())?;let page_count=document.PageCount().map_err(|e|e.to_string())?;
    let mut output=String::new();let mut seen=HashSet::new();for page_index in selected_page_indexes(page_count){let page=document.GetPage(page_index).map_err(|e|e.to_string())?;let size=page.Size().map_err(|e|e.to_string())?;let receipt_profile=receipt_hint||size.Height>size.Width*1.65;let regions=if receipt_profile{receipt_regions(size)}else{invoice_regions(size)};for region in regions{if let Ok(text)=ocr_region(&page,&engine,max_dimension,region){merge_lines(&mut output,&mut seen,&text)}}page.Close().map_err(|e|e.to_string())?}
    if output.trim().is_empty(){Err("La lecture OCR focalisée n'a produit aucun texte exploitable.".to_string())}else{Ok(output)}
}

fn already_attempted(path:&str)->bool{let set=FOCUSED_ATTEMPTS.get_or_init(||Mutex::new(HashSet::new()));match set.lock(){Ok(mut guard)=>!guard.insert(path.to_string()),Err(_)=>true}}

#[tauri::command]
pub fn optimize_focused_invoice_reading(app:AppHandle)->Result<FocusedOptimizationResult,String>{
    let connection=super::open_database(&app)?;let mut statement=connection.prepare("SELECT path,COALESCE(extracted_text,''),COALESCE(parsed_json,'') FROM invoices WHERE status='nouvelle' AND extraction_status='ocr_termine' ORDER BY updated_at ASC LIMIT 12").map_err(|e|e.to_string())?;
    let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;drop(statement);drop(connection);
    let mut result=FocusedOptimizationResult::default();
    for(path,text,json)in rows{result.inspected+=1;let parsed:super::ParsedInvoice=serde_json::from_str(&json).unwrap_or_default();if parsed.confidence>=99||already_attempted(&path){continue}let receipt_hint=super::receipt::is_receipt_like(&text);match ocr_focused_pdf(&path,receipt_hint){Ok(focused)=>{result.processed+=1;let merged=merge_texts(&text,&focused);if merged==text{continue}let augmented=super::receipt::augment_if_receipt(&merged);let reparsed=super::parse_invoice_text(&augmented);let new_json=serde_json::to_string(&reparsed).map_err(|e|e.to_string())?;let length=augmented.chars().filter(|c|!c.is_whitespace()).count() as i64;let db=super::open_database(&app)?;db.execute("UPDATE invoices SET extracted_text=?2,text_length=?3,parsed_json=?4,updated_at=CURRENT_TIMESTAMP WHERE path=?1",params![path,augmented,length,new_json]).map_err(|e|e.to_string())?;let _=super::record_audit(&db,Some(&path),"ocr_focused",Some(if receipt_hint{"receipt_regions"}else{"invoice_regions"}));result.improved+=1},Err(_)=>result.errors+=1}
        break;
    }
    Ok(result)
}

#[cfg(test)]mod tests{use super::{invoice_regions,receipt_regions,render_dimensions,selected_page_indexes};use windows::Foundation::Size;#[test]fn focused_render_respects_windows_limit(){assert_eq!(render_dimensions(500.0,1000.0,2000,3000.0),(1000,2000));}#[test]fn receipt_is_split_into_overlapping_bands(){let r=receipt_regions(Size{Width:300.0,Height:1000.0});assert_eq!(r.len(),6);assert!(r[0].Height>r[1].Y-r[0].Y);assert!(r.last().unwrap().Y+r.last().unwrap().Height<=1000.01)}#[test]fn invoice_targets_header_body_and_totals(){let r=invoice_regions(Size{Width:600.0,Height:840.0});assert_eq!(r.len(),5);assert!(r[0].Width>300.0);assert!(r[4].Y>400.0)}#[test]fn long_documents_focus_first_and_last_pages(){assert_eq!(selected_page_indexes(8),vec![0,1,6,7]);assert_eq!(selected_page_indexes(3),vec![0,1,2]);}}