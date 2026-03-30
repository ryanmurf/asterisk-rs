//! asterisk-apps: Dialplan application implementations.
//!
//! This crate provides the core dialplan applications that were traditionally
//! loaded as modules in Asterisk C. These include:
//!
//! - **Answer/Hangup** - Channel answer and hangup control
//! - **Dial** - The heart of Asterisk: parallel dialing, bridging, and call control
//! - **Playback** - Playing audio files to channels
//! - **Echo** - Echo test (read frames and write them back)
//! - **Record** - Recording audio from channels to files
//! - **ConfBridge** - Conference bridge (multi-party mixing)
//! - **Queue** - Call queue with strategy-based member selection
//! - **VoiceMail** - Voicemail leave/check system
//! - **Transfer** - Blind transfer a call
//! - **SoftHangup** - Request hangup on another channel
//! - **Originate** - Originate a new outbound call
//! - **Read** - Read DTMF digits from caller
//! - **System/TrySystem** - Execute system commands
//! - **SendText** - Send text message to channel
//! - **Verbose/Log/NoOp** - Logging and debugging
//! - **Wait/WaitExten/WaitDigit/WaitUntil** - Pause execution
//! - **GoSub/Return/StackPop** - Dialplan subroutines
//! - **Exec/TryExec/ExecIf** - Dynamic application execution
//! - **MixMonitor/StopMixMonitor** - Record/monitor active calls
//! - **ChanSpy/ExtenSpy** - Listen to active calls
//! - **Page** - Paging/intercom system
//! - **Directory** - Voice directory (dial by name)
//! - **BlindTransfer/AttnTransfer** - Blind and attended transfer
//! - **BridgeWait/BridgeAdd** - Holding bridge management
//! - **PrivacyManager** - Privacy/screening for anonymous calls
//! - **Authenticate** - Password authentication
//! - **MiniVM** - Mini voicemail building blocks
//! - **MP3Player** - MP3 playback via external decoder
//! - **DAHDIRas** - DAHDI remote access server
//! - **SMS** - SMS/text messaging over analog lines (ETSI ES 201 912)
//! - **AlarmReceiver** - Alarm system signals via Ademco Contact ID
//! - **AgentLogin/AgentRequest** - Call center agent pool management
//! - **Festival** - Festival TTS integration
//! - **JACK** - JACK audio connection (stub)
//! - **ICES** - Icecast streaming
//! - **NBScat** - NBS audio streaming
//! - **TestClient/TestServer** - Automated call testing
//! - **ChannelRedirect** - Redirect another channel
//! - **ControlPlayback** - Playback with DTMF controls
//! - **DBput/DBget/DBdel/DBdeltree** - AstDB access
//! - **DumpChan** - Dump channel info to log
//! - **SendDTMF/ReceiveDTMF** - Send/receive DTMF digits
//! - **ReadExten** - Read extension with real-time matching
//! - **Macro** - Legacy macro subroutine (deprecated)
//! - **While/EndWhile/ExitWhile/ContinueWhile** - Dialplan loops
//! - **GotoIf/GotoIfTime/If/ElseIf/Else/EndIf** - Conditional branching
//! - **Set/MSet** - Variable assignment
//! - **Goto** - Unconditional goto
//! - **SayUnixTime/DateTime** - Say date/time from timestamp
//! - **TDD** - TDD/TTY for hearing-impaired
//! - **AMD** - Answering Machine Detection
//! - **StatsD** - StatsD metrics from dialplan
//! - **ChanIsAvail** - Check channel availability
//! - **IVRDemo** - IVR demo skeleton
//! - **SendImage** - Send image to channel
//! - **ADSIProg** - ADSI script programming (stub)
//! - **UserEvent** - Custom AMI user events
//! - **WaitForRing** - Wait for ring event
//! - **WaitForSilence/WaitForNoise** - Wait for silence or noise
//! - **StreamEcho** - Multistream echo test

pub mod adapter;
pub mod answer;
pub mod hangup;
pub mod dial;
pub mod playback;
pub mod echo;
pub mod record;
pub mod confbridge;
pub mod queue;
pub mod voicemail;
pub mod registry;
pub mod transfer;
pub mod softhangup;
pub mod originate;
pub mod read;
pub mod system;
pub mod sendtext;
pub mod verbose;
pub mod wait;
pub mod stack;
pub mod exec;
pub mod mixmonitor;
pub mod chanspy;
pub mod page;
pub mod directory;
pub mod attended_transfer;
pub mod bridgewait;
pub mod privacy;
pub mod authenticate;
pub mod cdr_app;
pub mod celgenuserevent;
pub mod dictate;
pub mod disa;
pub mod external_ivr;
pub mod followme;
pub mod forkcdr;
pub mod milliwatt;
pub mod morsecode;
pub mod pickup;
pub mod playtones;
pub mod saycounted;
pub mod sla;
pub mod speech_utils;
pub mod url;
pub mod zapateller;
pub mod minivm;
pub mod mp3;
pub mod dahdiras;
pub mod sms;
pub mod alarmreceiver;
pub mod agent_pool;
pub mod agi_app;
pub mod festival;
pub mod jack;
pub mod ices;
pub mod nbscat;
pub mod test;
pub mod channelredirect;
pub mod controlplayback;
pub mod db;
pub mod dumpchan;
pub mod senddtmf;
pub mod readexten;
pub mod macro_;
pub mod while_;
pub mod if_;
pub mod set;
pub mod goto;
pub mod sayunixtime;
pub mod tdd;
pub mod amd;
pub mod statsd_app;
pub mod chanisavail;
pub mod ivrdemo;
pub mod image;
pub mod adsiprog;
pub mod userevent;
pub mod waitforring;
pub mod waitforsilence;
pub mod stream_echo;

pub use answer::AppAnswer;
pub use hangup::AppHangup;
pub use dial::{
    AppDial, DialStatus, DialOptions, DialArgs, DialDestination, DialplanLocation,
    CallLimit, DtmfSendSpec, AnnouncementSpec, DialBeginEvent, DialEndEvent,
};
pub use playback::AppPlayback;
pub use echo::AppEcho;
pub use record::AppRecord;
pub use confbridge::{
    AppConfBridge, Conference, ConferenceUser, UserProfile as ConfUserProfile,
    BridgeProfile as ConfBridgeProfile, ConfMenu, MenuAction, ConfBridgeResult,
    ConferenceVideoMode, ConferenceSettings, ConfBridgeEvent, ConferenceInfo,
};
pub use queue::{
    AppQueue, CallQueue, QueueMember, QueueCaller, QueueStrategy, QueueResult,
    QueueLogEvent, QueueLogEntry, QueueRule, QueueAnnouncements, QueueStatusVars,
    ConnectResult, MemberStatus, QueueOptions,
};
pub use voicemail::{
    AppVoiceMail, Mailbox, VoiceMessage, VoicemailFolder, VoicemailStatus,
    GreetingType, GreetingState, EmailNotification, PagerNotification,
    NotificationVars, RecordingConfig, PlaybackAction, PlaybackState,
    VoicemailStorage, FileStorage, ImapStorage, OdbcStorage,
    MailboxConfig, MwiState,
};
pub use registry::AppRegistry;
pub use transfer::AppTransfer;
pub use softhangup::AppSoftHangup;
pub use originate::AppOriginate;
pub use read::AppRead;
pub use system::{AppSystem, AppTrySystem};
pub use sendtext::AppSendText;
pub use verbose::{AppVerbose, AppLog, AppNoOp};
pub use wait::{AppWait, AppWaitExten, AppWaitDigit, AppWaitUntil};
pub use stack::{AppGoSub, AppReturn, AppStackPop, AppGoSubIf};
pub use exec::{AppExec, AppTryExec, AppExecIf};
pub use mixmonitor::{AppMixMonitor, AppStopMixMonitor, MixMonitorSession};
pub use chanspy::{AppChanSpy, AppExtenSpy, SpyMode, ChanSpyResult, SpyAudiohook};
pub use page::AppPage;
pub use directory::{AppDirectory, DirectoryEntry};
pub use attended_transfer::{AppBlindTransfer, AppAttnTransfer};
pub use bridgewait::{AppBridgeWait, AppBridgeAdd};
pub use privacy::AppPrivacyManager;
pub use authenticate::AppAuthenticate;
pub use cdr_app::AppResetCdr;
pub use celgenuserevent::AppCelGenUserEvent;
pub use dictate::AppDictate;
pub use disa::AppDisa;
pub use external_ivr::AppExternalIvr;
pub use followme::AppFollowMe;
pub use forkcdr::AppForkCdr;
pub use milliwatt::AppMilliwatt;
pub use morsecode::AppMorsecode;
pub use pickup::{AppPickup, AppPickupChan};
pub use playtones::{AppPlayTones, AppStopPlayTones};
pub use saycounted::{AppSayCountedNoun, AppSayCountedAdj};
pub use sla::{AppSlaStation, AppSlaTrunk};
pub use speech_utils::{
    AppSpeechCreate, AppSpeechActivateGrammar, AppSpeechStart, AppSpeechBackground,
    AppSpeechDeactivateGrammar, AppSpeechProcessingSound, AppSpeechDestroy,
    AppSpeechLoadGrammar, AppSpeechUnloadGrammar,
};
pub use url::AppSendUrl;
pub use zapateller::AppZapateller;
pub use minivm::{AppMinivmRecord, AppMinivmGreet, AppMinivmNotify, AppMinivmDelete, AppMinivmAccMess};
pub use mp3::AppMp3Player;
pub use dahdiras::AppDahdiRas;
pub use sms::AppSms;
pub use alarmreceiver::AppAlarmReceiver;
pub use agent_pool::{AppAgentLogin, AppAgentRequest, AgentPool, AGENT_POOL};
pub use agi_app::AppAgi;
pub use festival::AppFestival;
pub use jack::AppJack;
pub use ices::AppIces;
pub use nbscat::AppNbscat;
pub use test::{AppTestServer, AppTestClient};
pub use channelredirect::AppChannelRedirect;
pub use controlplayback::AppControlPlayback;
pub use db::{AppDbPut, AppDbGet, AppDbDel, AppDbDelTree};
pub use dumpchan::AppDumpChan;
pub use senddtmf::{AppSendDtmf, AppReceiveDtmf};
pub use readexten::AppReadExten;
pub use macro_::{AppMacro, AppMacroExclusive, AppMacroExit, AppMacroIf};
pub use while_::{AppWhile, AppEndWhile, AppExitWhile, AppContinueWhile};
pub use if_::{AppGotoIf, AppGotoIfTime, AppIf, AppElseIf, AppElse, AppEndIf};
pub use set::{AppSet, AppMSet};
pub use goto::AppGoto;
pub use sayunixtime::{AppSayUnixTime, AppDateTime};
pub use tdd::AppTdd;
pub use amd::AppAmd;
pub use statsd_app::AppStatsd;
pub use chanisavail::AppChanIsAvail;
pub use ivrdemo::AppIvrDemo;
pub use image::AppSendImage;
pub use adsiprog::AppAdsiProg;
pub use userevent::AppUserEvent;
pub use waitforring::AppWaitForRing;
pub use waitforsilence::{AppWaitForSilence, AppWaitForNoise};
pub use stream_echo::AppStreamEcho;

/// Result returned by a dialplan application execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PbxExecResult {
    /// Application executed successfully, continue to next priority.
    Success,
    /// Application failed, but continue execution.
    Failed,
    /// Channel was hung up during application execution.
    Hangup,
}

/// Trait that all dialplan applications implement.
pub trait DialplanApp: Send + Sync {
    /// The name of the application as used in extensions.conf.
    fn name(&self) -> &str;

    /// A short description of the application.
    fn description(&self) -> &str;
}
