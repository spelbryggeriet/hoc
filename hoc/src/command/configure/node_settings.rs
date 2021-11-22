use super::*;

impl Configure {
    pub(super) fn configure_node_settings(
        &self,
        step: &mut ProcedureStep,
        local_endpoint: LocalEndpoint,
    ) -> Result<Halt<ConfigureState>> {
        Ok(Halt::persistent_finish())
    }
}
