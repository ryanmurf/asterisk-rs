//! Application registry - tracks all registered dialplan applications.

use crate::DialplanApp;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of all available dialplan applications.
///
/// Applications register themselves here so the PBX can look them up
/// when executing dialplan extensions.
pub struct AppRegistry {
    apps: RwLock<HashMap<String, Arc<dyn DialplanApp>>>,
}

impl AppRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            apps: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry pre-populated with all built-in applications.
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        registry.register(Arc::new(crate::answer::AppAnswer));
        registry.register(Arc::new(crate::hangup::AppHangup));
        registry.register(Arc::new(crate::dial::AppDial));
        registry.register(Arc::new(crate::playback::AppPlayback));
        registry.register(Arc::new(crate::echo::AppEcho));
        registry.register(Arc::new(crate::record::AppRecord));
        registry.register(Arc::new(crate::confbridge::AppConfBridge));
        registry.register(Arc::new(crate::queue::AppQueue));
        registry.register(Arc::new(crate::voicemail::AppVoiceMail));
        registry.register(Arc::new(crate::transfer::AppTransfer));
        registry.register(Arc::new(crate::softhangup::AppSoftHangup));
        registry.register(Arc::new(crate::originate::AppOriginate));
        registry.register(Arc::new(crate::read::AppRead));
        registry.register(Arc::new(crate::system::AppSystem));
        registry.register(Arc::new(crate::system::AppTrySystem));
        registry.register(Arc::new(crate::sendtext::AppSendText));
        registry.register(Arc::new(crate::verbose::AppVerbose));
        registry.register(Arc::new(crate::verbose::AppLog));
        registry.register(Arc::new(crate::verbose::AppNoOp));
        registry.register(Arc::new(crate::wait::AppWait));
        registry.register(Arc::new(crate::wait::AppWaitExten));
        registry.register(Arc::new(crate::wait::AppWaitDigit));
        registry.register(Arc::new(crate::wait::AppWaitUntil));
        registry.register(Arc::new(crate::stack::AppGoSub));
        registry.register(Arc::new(crate::stack::AppGoSubIf));
        registry.register(Arc::new(crate::stack::AppReturn));
        registry.register(Arc::new(crate::stack::AppStackPop));
        registry.register(Arc::new(crate::exec::AppExec));
        registry.register(Arc::new(crate::exec::AppTryExec));
        registry.register(Arc::new(crate::exec::AppExecIf));
        registry.register(Arc::new(crate::mixmonitor::AppMixMonitor));
        registry.register(Arc::new(crate::mixmonitor::AppStopMixMonitor));
        registry.register(Arc::new(crate::chanspy::AppChanSpy));
        registry.register(Arc::new(crate::chanspy::AppExtenSpy));
        registry.register(Arc::new(crate::page::AppPage));
        registry.register(Arc::new(crate::directory::AppDirectory));
        registry.register(Arc::new(crate::attended_transfer::AppBlindTransfer));
        registry.register(Arc::new(crate::attended_transfer::AppAttnTransfer));
        registry.register(Arc::new(crate::bridgewait::AppBridgeWait));
        registry.register(Arc::new(crate::bridgewait::AppBridgeAdd));
        registry.register(Arc::new(crate::privacy::AppPrivacyManager));
        registry.register(Arc::new(crate::authenticate::AppAuthenticate));
        registry.register(Arc::new(crate::cdr_app::AppResetCdr));
        registry.register(Arc::new(crate::celgenuserevent::AppCelGenUserEvent));
        registry.register(Arc::new(crate::dictate::AppDictate));
        registry.register(Arc::new(crate::disa::AppDisa));
        registry.register(Arc::new(crate::external_ivr::AppExternalIvr));
        registry.register(Arc::new(crate::followme::AppFollowMe));
        registry.register(Arc::new(crate::forkcdr::AppForkCdr));
        registry.register(Arc::new(crate::milliwatt::AppMilliwatt));
        registry.register(Arc::new(crate::morsecode::AppMorsecode));
        registry.register(Arc::new(crate::pickup::AppPickup));
        registry.register(Arc::new(crate::pickup::AppPickupChan));
        registry.register(Arc::new(crate::playtones::AppPlayTones));
        registry.register(Arc::new(crate::playtones::AppStopPlayTones));
        registry.register(Arc::new(crate::saycounted::AppSayCountedNoun));
        registry.register(Arc::new(crate::saycounted::AppSayCountedAdj));
        registry.register(Arc::new(crate::sla::AppSlaStation));
        registry.register(Arc::new(crate::sla::AppSlaTrunk));
        registry.register(Arc::new(crate::speech_utils::AppSpeechCreate));
        registry.register(Arc::new(crate::speech_utils::AppSpeechActivateGrammar));
        registry.register(Arc::new(crate::speech_utils::AppSpeechStart));
        registry.register(Arc::new(crate::speech_utils::AppSpeechBackground));
        registry.register(Arc::new(crate::speech_utils::AppSpeechDeactivateGrammar));
        registry.register(Arc::new(crate::speech_utils::AppSpeechProcessingSound));
        registry.register(Arc::new(crate::speech_utils::AppSpeechDestroy));
        registry.register(Arc::new(crate::speech_utils::AppSpeechLoadGrammar));
        registry.register(Arc::new(crate::speech_utils::AppSpeechUnloadGrammar));
        registry.register(Arc::new(crate::url::AppSendUrl));
        registry.register(Arc::new(crate::zapateller::AppZapateller));
        registry.register(Arc::new(crate::minivm::AppMinivmRecord));
        registry.register(Arc::new(crate::minivm::AppMinivmGreet));
        registry.register(Arc::new(crate::minivm::AppMinivmNotify));
        registry.register(Arc::new(crate::minivm::AppMinivmDelete));
        registry.register(Arc::new(crate::minivm::AppMinivmAccMess));
        registry.register(Arc::new(crate::mp3::AppMp3Player));
        registry.register(Arc::new(crate::dahdiras::AppDahdiRas));
        registry.register(Arc::new(crate::sms::AppSms));
        registry.register(Arc::new(crate::alarmreceiver::AppAlarmReceiver));
        registry.register(Arc::new(crate::agent_pool::AppAgentLogin));
        registry.register(Arc::new(crate::agent_pool::AppAgentRequest));
        registry.register(Arc::new(crate::festival::AppFestival));
        registry.register(Arc::new(crate::jack::AppJack));
        registry.register(Arc::new(crate::ices::AppIces));
        registry.register(Arc::new(crate::nbscat::AppNbscat));
        registry.register(Arc::new(crate::test::AppTestServer));
        registry.register(Arc::new(crate::test::AppTestClient));
        registry.register(Arc::new(crate::channelredirect::AppChannelRedirect));
        registry.register(Arc::new(crate::controlplayback::AppControlPlayback));
        registry.register(Arc::new(crate::db::AppDbPut));
        registry.register(Arc::new(crate::db::AppDbGet));
        registry.register(Arc::new(crate::db::AppDbDel));
        registry.register(Arc::new(crate::db::AppDbDelTree));
        registry.register(Arc::new(crate::dumpchan::AppDumpChan));
        registry.register(Arc::new(crate::senddtmf::AppSendDtmf));
        registry.register(Arc::new(crate::senddtmf::AppReceiveDtmf));
        registry.register(Arc::new(crate::readexten::AppReadExten));
        registry.register(Arc::new(crate::macro_::AppMacro));
        registry.register(Arc::new(crate::macro_::AppMacroExclusive));
        registry.register(Arc::new(crate::macro_::AppMacroExit));
        registry.register(Arc::new(crate::macro_::AppMacroIf));
        registry.register(Arc::new(crate::while_::AppWhile));
        registry.register(Arc::new(crate::while_::AppEndWhile));
        registry.register(Arc::new(crate::while_::AppExitWhile));
        registry.register(Arc::new(crate::while_::AppContinueWhile));
        registry.register(Arc::new(crate::if_::AppGotoIf));
        registry.register(Arc::new(crate::if_::AppGotoIfTime));
        registry.register(Arc::new(crate::if_::AppIf));
        registry.register(Arc::new(crate::if_::AppElseIf));
        registry.register(Arc::new(crate::if_::AppElse));
        registry.register(Arc::new(crate::if_::AppEndIf));
        registry.register(Arc::new(crate::set::AppSet));
        registry.register(Arc::new(crate::set::AppMSet));
        registry.register(Arc::new(crate::goto::AppGoto));
        registry.register(Arc::new(crate::sayunixtime::AppSayUnixTime));
        registry.register(Arc::new(crate::sayunixtime::AppDateTime));
        registry.register(Arc::new(crate::tdd::AppTdd));
        registry.register(Arc::new(crate::amd::AppAmd));
        registry.register(Arc::new(crate::statsd_app::AppStatsd));
        registry.register(Arc::new(crate::chanisavail::AppChanIsAvail));
        registry.register(Arc::new(crate::ivrdemo::AppIvrDemo));
        registry.register(Arc::new(crate::image::AppSendImage));
        registry.register(Arc::new(crate::adsiprog::AppAdsiProg));
        registry.register(Arc::new(crate::userevent::AppUserEvent));
        registry.register(Arc::new(crate::waitforring::AppWaitForRing));
        registry.register(Arc::new(crate::waitforsilence::AppWaitForSilence));
        registry.register(Arc::new(crate::waitforsilence::AppWaitForNoise));
        registry.register(Arc::new(crate::stream_echo::AppStreamEcho));
        registry
    }

    /// Register a dialplan application.
    pub fn register(&self, app: Arc<dyn DialplanApp>) {
        let name = app.name().to_string();
        tracing::debug!("AppRegistry: registering application '{}'", name);
        self.apps.write().insert(name, app);
    }

    /// Look up an application by name (case-sensitive).
    pub fn get(&self, name: &str) -> Option<Arc<dyn DialplanApp>> {
        self.apps.read().get(name).cloned()
    }

    /// List all registered application names.
    pub fn list(&self) -> Vec<String> {
        let apps = self.apps.read();
        let mut names: Vec<String> = apps.keys().cloned().collect();
        names.sort();
        names
    }

    /// Get the count of registered applications.
    pub fn count(&self) -> usize {
        self.apps.read().len()
    }

    /// Unregister an application by name.
    pub fn unregister(&self, name: &str) -> bool {
        self.apps.write().remove(name).is_some()
    }
}

impl Default for AppRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_builtins() {
        let registry = AppRegistry::with_builtins();
        assert!(registry.count() >= 9);

        assert!(registry.get("Answer").is_some());
        assert!(registry.get("Hangup").is_some());
        assert!(registry.get("Dial").is_some());
        assert!(registry.get("Playback").is_some());
        assert!(registry.get("Echo").is_some());
        assert!(registry.get("Record").is_some());
        assert!(registry.get("ConfBridge").is_some());
        assert!(registry.get("Queue").is_some());
        assert!(registry.get("VoiceMail").is_some());
    }

    #[test]
    fn test_registry_list() {
        let registry = AppRegistry::with_builtins();
        let names = registry.list();
        assert!(names.contains(&"Dial".to_string()));
    }

    #[test]
    fn test_registry_unregister() {
        let registry = AppRegistry::with_builtins();
        let before = registry.count();
        assert!(registry.unregister("Echo"));
        assert_eq!(registry.count(), before - 1);
        assert!(registry.get("Echo").is_none());
    }
}
