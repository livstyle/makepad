use {
    std::ptr,
    std::mem,
    std::sync::{Arc, Mutex},
    crate::{
        platform::apple::frameworks::*,
        platform::apple::cocoa_app::*,
        platform::apple::apple_util::*,
        objc_block,
    },
};

pub struct MidiEndpoint {
    pub id: i32,
    pub name: String,
    pub manufacturer: String,
    endpoint: MIDIEndpointRef
}

pub struct Midi1Event {
    input: usize,
    status: u8,
    data1: u8,
    data2: u8
}

pub struct Instrument {
    object: ObjcId
}

pub struct Midi {
    pub sources: Vec<MidiEndpoint>,
    pub destinations: Vec<MidiEndpoint>
}

impl MidiEndpoint {
    unsafe fn new(endpoint: MIDIEndpointRef) -> Result<Self,
    OSError> {
        let mut manufacturer = 0 as CFStringRef;
        let mut name = 0 as CFStringRef;
        let mut id = 0i32;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyManufacturer, &mut manufacturer)) ?;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut name)) ?;
        OSError::from(MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut id)) ?;
        Ok(Self {
            id,
            name: cfstring_ref_to_string(name),
            manufacturer: cfstring_ref_to_string(manufacturer),
            endpoint
        })
    }
}

impl Midi {
    pub fn new_midi_1_input(message_callback: Box<dyn Fn(Midi1Event)>,) -> Result<Self,
    OSError> {
        let mut midi_notify = objc_block!(move | _notification: &MIDINotification | {
            println!("Midi device added/removed");
        });
        
        let mut midi_receive = objc_block!(move | event_list: &MIDIEventList, user_data: u64 | {
            let packets = unsafe {std::slice::from_raw_parts(event_list.packet.as_ptr(), event_list.numPackets as usize)};
            for packet in packets {
                for i in 0 .. packet.wordCount {
                    let ump = packet.words[i as usize];
                    let ty = ((ump & 0xf000_0000) >> 28) as u8;
                    let _group = ((ump & 0x0f00_0000) >> 24) as u8;
                    let status = ((ump & 0x00ff_0000) >> 16) as u8;
                    let data1 = ((ump & 0x0000_ff00) >> 8) as u8;
                    let data2 = ((ump & 0x0000_00ff) >> 0) as u8;
                    if ty == 0x02 { // midi 1.0 channel voice
                        message_callback(Midi1Event {
                            input: user_data as usize,
                            status,
                            data1,
                            data2
                        })
                    }
                }
            }
        });
        
        let mut midi_client = 0 as MIDIClientRef;
        let mut midi_in_port = 0 as MIDIPortRef;
        let mut midi_out_port = 0 as MIDIPortRef;
        let mut destinations = Vec::new();
        let mut sources = Vec::new();
        unsafe {
            OSError::from(MIDIClientCreateWithBlock(
                ccfstr_from_str("Makepad"),
                &mut midi_client,
                &mut midi_notify as *mut _ as ObjcId
            )) ?;
            
            OSError::from(MIDIInputPortCreateWithProtocol(
                midi_client,
                ccfstr_from_str("MIDI Input"),
                kMIDIProtocol_1_0,
                &mut midi_in_port,
                &mut midi_receive as *mut _ as ObjcId
            )) ?;
            
            OSError::from(MIDIOutputPortCreate(
                midi_client,
                ccfstr_from_str("MIDI Output"),
                &mut midi_out_port
            )) ?;
            
            for i in 0..MIDIGetNumberOfDestinations() {
                if let Ok(ep) = MidiEndpoint::new(MIDIGetDestination(i)) {
                    destinations.push(ep);
                }
            }
            for i in 0..MIDIGetNumberOfSources() {
                if let Ok(ep) = MidiEndpoint::new(MIDIGetSource(i)) {
                    MIDIPortConnectSource(midi_in_port, ep.endpoint, i as *mut _);
                    sources.push(ep);
                }
            }
        }
        
        Ok(Self {
            sources,
            destinations
        })
    }
}

pub struct Audio {}

#[derive(Clone)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub device_type: AudioDeviceType,
    desc: AudioComponentDescription
}

#[derive(Copy, Clone)]
pub enum AudioDeviceType {
    DefaultOutput,
    Music
}

unsafe impl Send for AudioDevice {}
pub struct AudioDevice {
    av_audio_unit: ObjcId,
    au_audio_unit: ObjcId,
    render_block: Option<ObjcId>,
    view_controller: Arc<Mutex<Option<ObjcId>>>,
    device_type: AudioDeviceType
}

pub struct AudioBuffer<'a> {
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
    
    buffers: *mut AudioBufferList,
    flags: *mut u32,
    timestamp: *const AudioTimeStamp,
    frame_count: u32,
    input_bus_number: u64,
}

impl AudioDevice {
    pub fn start_audio_output_with_fn(&self, audio_callback: Box<dyn Fn(&mut AudioBuffer) + Send>) {
        match self.device_type {
            AudioDeviceType::DefaultOutput => (),
            _ => panic!("start_audio_output_with_fn on this device")
        }
        unsafe {
            let output_provider = objc_block!(
                move | flags: *mut u32,
                timestamp: *const AudioTimeStamp,
                frame_count: u32,
                input_bus_number: u64,
                buffers: *mut AudioBufferList |: i32 {
                    let buffers_ref = &*buffers;
                    audio_callback(&mut AudioBuffer {
                        left: std::slice::from_raw_parts_mut(
                            buffers_ref.mBuffers[0].mData as *mut f32,
                            frame_count as usize
                        ),
                        right: std::slice::from_raw_parts_mut(
                            buffers_ref.mBuffers[1].mData as *mut f32,
                            frame_count as usize
                        ),
                        buffers,
                        flags,
                        timestamp,
                        frame_count,
                        input_bus_number
                    });
                    0
                }
            );
            let () = msg_send![self.au_audio_unit, setOutputProvider: &output_provider];
        }
    }

    pub fn render_to_audio_buffer(&self, buffer:&mut AudioBuffer) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("render_to_audio_buffer not supported on this device")
        }
        if let Some(render_block) = self.render_block {
            unsafe{objc_block_invoke!(render_block, invoke(
                (buffer.flags): *mut u32,
                (buffer.timestamp): *const AudioTimeStamp,
                (buffer.frame_count): u32,
                (buffer.input_bus_number): u64,
                (buffer.buffers): *mut AudioBufferList,
                (nil): ObjcId
            ) -> i32)};
        }
    }
    
    
    pub fn request_ui(&self, view_loaded: Box<dyn Fn() + Send>) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("request_ui not supported on this device")
        }
        
        let view_controller_arc = self.view_controller.clone();
        unsafe{
            let view_controller_complete = objc_block!(move | view_controller: ObjcId | {
                *view_controller_arc.lock().unwrap() = Some(view_controller);
                view_loaded();
            });
            
            let () = msg_send![self.au_audio_unit, requestViewControllerWithCompletionHandler: &view_controller_complete];
        }
    }
    
    pub fn open_ui(&self){
        if let Some(view_controller) = self.view_controller.lock().unwrap().as_ref(){
            unsafe{
                let audio_view: ObjcId = msg_send![*view_controller, view];
                let cocoa_app = get_cocoa_app_global();
                let win_view = cocoa_app.cocoa_windows[0].1;
                let () = msg_send![win_view, addSubview: audio_view];
            }
        }
    }
    
}

#[derive(Debug)]
pub enum AudioError {
    System(String),
    NoDevice
}

impl Audio {
    
    pub fn query_devices(device_type: AudioDeviceType) -> Vec<AudioDeviceInfo> {
        unsafe {
            let desc = match device_type {
                AudioDeviceType::Music => {
                    AudioComponentDescription::new_all_manufacturers(
                        AudioUnitType::MusicDevice,
                        AudioUnitSubType::Undefined,
                    )
                }
                AudioDeviceType::DefaultOutput => {
                    AudioComponentDescription::new_apple(
                        AudioUnitType::IO,
                        AudioUnitSubType::DefaultOutput,
                    )
                }
            };
            
            let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
            let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
            let count: usize = msg_send![components, count];
            let mut out = Vec::new();
            for i in 0..count {
                let component: ObjcId = msg_send![components, objectAtIndex: i];
                let name = nsstring_to_string(msg_send![component, name]);
                let desc: AudioComponentDescription = msg_send!(component, audioComponentDescription);
                out.push(AudioDeviceInfo {device_type, name, desc});
            }
            out
        }
    }
    
    pub fn new_device(
        device_info: &AudioDeviceInfo,
        device_callback: Box<dyn Fn(Result<AudioDevice, AudioError>) + Send>,
    ) {
        unsafe {
            /*
            let view_controller_complete = objc_block!(move | view_controller: ObjcId | {
                vc(view_controller as u64);
            });
            */
            let device_type = device_info.device_type;
            let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
                unsafe fn inner(av_audio_unit: ObjcId, error: ObjcId, device_type: AudioDeviceType) -> Result<AudioDevice, OSError> {
                    OSError::from_nserror(error) ?;
                    let au_audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
                    
                    let mut err: ObjcId = nil;
                    let () = msg_send![au_audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                    OSError::from_nserror(err) ?;
                    let mut render_block = None;
                    match device_type {
                        AudioDeviceType::DefaultOutput => {
                            let () = msg_send![au_audio_unit, setOutputEnabled: true];
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, startHardwareAndReturnError: &mut err];
                            OSError::from_nserror(err) ?;
                        }
                        AudioDeviceType::Music => {
                            let block_ptr: ObjcId = msg_send![au_audio_unit, renderBlock];
                            let () = msg_send![block_ptr, retain];
                            render_block = Some(block_ptr);
                        }
                        _ => ()
                    }
                    
                    Ok(AudioDevice {
                        view_controller:Arc::new(Mutex::new(None)),
                        render_block,
                        device_type,
                        av_audio_unit,
                        au_audio_unit
                    })
                }
                
                match inner(av_audio_unit, error, device_type) {
                    Err(err) => device_callback(Err(AudioError::System(format!("{:?}", err)))),
                    Ok(device) => device_callback(Ok(device))
                }
            });
            
            // Instantiate output audio unit
            let () = msg_send![
                class!(AVAudioUnit),
                instantiateWithComponentDescription: device_info.desc
                options: kAudioComponentInstantiation_LoadOutOfProcess
                completionHandler: &instantiation_handler
            ];
        }
    }
    /*
    pub fn new_audio_output_from_desc(desc: AudioComponentDescription, audio_callback: Box<dyn Fn(&mut [f32], &mut [f32]) -> Option<u64 >>) {
        unsafe{
            let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
            let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
            let count: usize = msg_send![components, count];
            if count != 1 {
                panic!();
            }
            
            let component: ObjcId = msg_send![components, objectAtIndex: 0];
            let desc: AudioComponentDescription = msg_send![component, audioComponentDescription];
            
            let output_provider = objc_block!(
                move | flags: *mut u32,
                timestamp: *const AudioTimeStamp,
                frame_count: u32,
                input_bus_number: u64,
                buffers: *mut AudioBufferList |: i32 {
                    let buffers_ref = &*buffers;
                    let left_chan = std::slice::from_raw_parts_mut(
                        buffers_ref.mBuffers[0].mData as *mut f32,
                        frame_count as usize
                    );
                    let right_chan = std::slice::from_raw_parts_mut(
                        buffers_ref.mBuffers[1].mData as *mut f32,
                        frame_count as usize
                    );
                    let block_ptr = audio_callback(left_chan, right_chan);
                    if let Some(block_ptr) = block_ptr {
                        objc_block_invoke!(block_ptr, invoke(
                            flags: *mut u32,
                            timestamp: *const AudioTimeStamp,
                            frame_count: u32,
                            input_bus_number: u64,
                            buffers: *mut AudioBufferList,
                            nil: ObjcId
                        ) -> i32);
                    }
                    0
                }
            );
            
            let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
                // lets spawn a thread
                OSError::from_nserror(error).expect("instantiateWithComponentDescription");
                
                let audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
                
                let () = msg_send![audio_unit, setOutputProvider: &output_provider];
                let () = msg_send![audio_unit, setOutputEnabled: true];
                
                let mut err: ObjcId = nil;
                let () = msg_send![audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                OSError::from_nserror(err).expect("allocateRenderResourcesAndReturnError");
                
                let mut err: ObjcId = nil;
                let () = msg_send![audio_unit, startHardwareAndReturnError: &mut err];
                OSError::from_nserror(err).expect("startHardwareAndReturnError");
                // stay in a waitloop so the audio output gets callbacks.
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
            
            // Instantiate output audio unit
            let () = msg_send![
                class!(AVAudioUnit),
                instantiateWithComponentDescription: desc
                options: kAudioComponentInstantiation_LoadInProcess
                completionHandler: &instantiation_handler
            ];
        }
    }*/
}