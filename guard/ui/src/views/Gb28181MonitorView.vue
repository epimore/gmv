<template>
  <div v-if="!selectedDevice && monitorMode === 'devices'" class="page-grid" v-loading="loading">
    <GlassPanel class="span-12" title="监控信息" subtitle="按设备查看在线状态和注册时间">
      <div class="toolbar">
        <el-select v-model="selectedListNodeId" filterable placeholder="选择 Session 节点" style="width: 420px"
          :loading="listNodeLoading" @change="handleListNodeChange">
          <el-option v-for="option in sessionNodeOptions" :key="option.node.node_id" :label="listNodeLabel(option)"
            :value="option.node.node_id" :disabled="option.disabled">
            <div class="node-option" :class="{ offline: option.disabled }">
              <span>{{ option.kindLabel }} · {{ option.node.node_id }}</span>
              <span class="node-status">{{ option.statusLabel }}</span>
            </div>
          </el-option>
        </el-select>
        <el-input v-model="deviceName" style="width: 220px" clearable placeholder="设备名称" @clear="queryDevices" />
        <el-button type="primary" :loading="loading" @click="queryDevices">查询</el-button>
        <el-button :loading="loading" @click="resetDevices">重置</el-button>
        <!-- <el-button :loading="loading" @click="loadDevices">刷新</el-button> -->
        <el-button type="primary" plain @click="openMultiView('live')">多画面工作台</el-button>
      </div>
      <el-table :data="devices" height="620" empty-text="暂无监控设备">
        <el-table-column type="index" :index="tableIndex" label="序号" width="64" />
        <el-table-column prop="device_id" label="设备 ID" min-width="200" show-overflow-tooltip />
        <el-table-column label="设备名称" min-width="160" show-overflow-tooltip>
          <template #default="{ row }">{{ displayDeviceName(row) }}</template>
        </el-table-column>
        <el-table-column label="状态" width="100">
          <template #default="{ row }">
            <StatusPill :label="row.monitor_status === 1 ? '在线' : '离线'"
              :tone="row.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
          </template>
        </el-table-column>
        <el-table-column label="国标版本" width="100">
          <template #default="{ row }">{{ emptyText(row.gb_version) }}</template>
        </el-table-column>
        <el-table-column label="注册时间" min-width="170" show-overflow-tooltip>
          <template #default="{ row }">{{ row.register_time || '-' }}</template>
        </el-table-column>
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button type="primary" link @click="openDeviceDetail(row)">查看</el-button>
            <el-button type="primary" link @click="openChannels(row)">相机</el-button>
          </template>
        </el-table-column>
      </el-table>
      <div class="pagination-bar">
        <el-pagination v-model:current-page="page" v-model:page-size="pageSize" :total="total"
          :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next, jumper" @current-change="loadDevices"
          @size-change="handlePageSizeChange" />
      </div>
    </GlassPanel>

    <el-drawer v-model="deviceDetailDrawer" :title="deviceDetailTitle" size="520px" class="device-detail-drawer"
      destroy-on-close>
      <div v-if="detailDevice" class="device-detail">
        <div class="detail-row">
          <div class="detail-item wide"><span>设备 ID</span><b>{{ detailDevice.device_id }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>设备名称</span><b>{{ displayDeviceName(detailDevice) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>注册时间</span><b>{{ detailDevice.register_time || '-' }}</b></div>
        </div>
        <div class="detail-row two">
          <div class="detail-item"><span>类型</span><b>{{ emptyText(detailDevice.device_type) }}</b></div>
          <div class="detail-item"><span>国标版本</span><b>{{ emptyText(detailDevice.gb_version) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide">
            <span>状态</span>
            <b>
              <StatusPill :label="detailDevice.monitor_status === 1 ? '在线' : '离线'"
                :tone="detailDevice.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
            </b>
          </div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>路数</span><b>{{ countText(detailDevice.max_camera) }}</b></div>
        </div>
        <div class="detail-row two">
          <div class="detail-item"><span>（接入）在线</span><b>{{ detailDevice.camera_in_count }}</b></div>
          <div class="detail-item"><span>离线</span><b>{{ detailDevice.camera_off_count }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>型号</span><b>{{ emptyText(detailDevice.model) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>固件版本</span><b>{{ emptyText(detailDevice.firmware) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>厂家</span><b>{{ emptyText(detailDevice.manufacturer) }}</b></div>
        </div>
      </div>
      <template #footer>
        <el-button @click="deviceDetailDrawer = false">关闭</el-button>
        <el-button v-if="detailDevice" type="primary" @click="openChannelsFromDetail">相机</el-button>
      </template>
    </el-drawer>
  </div>

  <div v-else-if="!selectedDevice" class="page-grid">
    <GlassPanel class="span-12">
      <div class="monitor-head">
        <div class="device-summary">
          <strong>多画面工作台</strong>
          <el-radio-group :model-value="multiMode" size="small" @change="handleMultiModeChange">
            <el-radio-button value="live">实时直播</el-radio-button>
            <el-radio-button value="playback">历史回放</el-radio-button>
          </el-radio-group>
        </div>
        <div class="monitor-actions">
          <el-date-picker v-if="multiMode === 'playback'" v-model="multiDefaultRange" type="datetimerange"
            range-separator="至" start-placeholder="默认开始时间" end-placeholder="默认结束时间"
            :clearable="true" class="multi-default-range" />
          <el-select v-model="selectedMultiNodeId" filterable placeholder="选择 Session 节点" class="multi-node-select"
            :loading="listNodeLoading" @change="selectMultiNode">
            <el-option v-for="option in sessionNodeOptions" :key="option.node.node_id" :label="listNodeLabel(option)"
              :value="option.node.node_id" :disabled="option.disabled">
              <div class="node-option" :class="{ offline: option.disabled }">
                <span>{{ option.kindLabel }} · {{ option.node.node_id }}</span>
                <span class="node-status">{{ option.statusLabel }}</span>
              </div>
            </el-option>
          </el-select>
          <el-button type="primary" @click="backToDeviceListFromMulti">返回设备列表</el-button>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-5" title="设备与通道">
      <div v-if="selectedMultiNodeOption" class="tree-workbench">
        <div class="toolbar">
          <el-input v-model="treeDeviceId" style="width: 220px" clearable placeholder="设备 ID" />
          <el-input v-model="treeDeviceName" style="width: 220px" clearable placeholder="设备名称" />
          <el-button type="primary" :loading="treeLoading" @click="searchTreeDevices">查询</el-button>
          <el-button :loading="treeLoading || multiStopping" @click="resetTreeDevices">重置</el-button>
        </div>
        <div v-loading="treeLoading" class="tree-device-list">
          <el-tree ref="multiDeviceTreeRef" class="device-channel-tree" :data="treeDeviceNodes" :props="treeProps" node-key="key" lazy
            :load="loadTreeNode" accordion :expand-on-click-node="true" :highlight-current="true">
            <template #default="{ data }">
              <div v-if="data.kind === 'device'" class="tree-device-node">
                <div class="tree-device-title">
                  <b>{{ data.device.device_id }} · {{ data.label }}</b>
                </div>
                <StatusPill :label="data.device.monitor_status === 1 ? '在线' : '离线'"
                  :tone="data.device.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
              </div>
              <el-checkbox v-else class="tree-channel-node" :model-value="selectedTreeChannelKeys.includes(data.key)"
                :disabled="!canSelectMultiChannel(data.channel)" @click.stop
                @change="(checked: boolean) => toggleTreeChannel(data.channel, checked)">
                <span class="tree-channel-label">
                  <b>{{ data.label }}</b>
                  <small>{{ data.channel.channel_id }} · {{ channelStatusText(data.channel) }}</small>
                </span>
              </el-checkbox>
            </template>
          </el-tree>
          <el-empty v-if="!treeLoading && !treeDevices.length" description="暂无设备" />
        </div>
        <div class="pagination-bar tree-pagination">
          <el-pagination v-model:current-page="treePage" v-model:page-size="treePageSize" :total="treeTotal"
            :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next" @current-change="queryTreeDevices"
            @size-change="handleTreePageSizeChange" />
        </div>
      </div>
      <el-empty v-else description="请选择信令节点" />
    </GlassPanel>

    <GlassPanel class="span-7" title="已选通道">
      <template #action>
        <div class="selected-channel-capacity">
          <span>{{ selectedTreeChannels.length }} / {{ multiViewLimit }}</span>
          <el-tooltip v-if="multiViewLimit === 6" :visible="multiLimitHelpVisible"
            content="当前访问链路未确认同时满足 HTTPS 与 HTTP/2，系统按 HTTP/1.1 的安全上限限制为 6 路；使用 HTTPS + HTTP/2 后，上限可提升至 16 路。"
            placement="top-end">
            <button type="button" class="multi-limit-help" aria-label="查看多画面上限为 6 路的原因"
              @mouseenter="multiLimitHelpHovered = true" @mouseleave="multiLimitHelpHovered = false"
              @click="multiLimitHelpPinned = !multiLimitHelpPinned" @blur="multiLimitHelpPinned = false">
              <el-icon><QuestionFilled /></el-icon>
            </button>
          </el-tooltip>
        </div>
      </template>
      <div class="selected-channel-panel">
        <div class="selected-channel-list" :class="{ playback: multiMode === 'playback' }">
          <article v-for="(channel, index) in selectedTreeChannels" :key="selectedChannelKey(channel)"
            class="selected-channel-item" :class="{ dragging: draggingTreeChannelIndex === index }" draggable="true"
            @dragstart="handleSelectedChannelDragStart(index)" @dragover.prevent @drop="handleSelectedChannelDrop(index)"
            @dragend="handleSelectedChannelDragEnd">
            <div class="selected-channel-main" @click="focusSelectedMultiChannel(channel)">
              <span v-if="multiMode === 'playback'" class="selected-channel-index">{{ index + 1 }}.</span>
              <el-tooltip :content="selectedChannelTooltip(channel)" placement="top">
                <b v-if="multiMode === 'playback'">{{ channel.device_id }} · {{ channel.channel_id }}</b>
                <b v-else>{{ index + 1 }}. {{ channel.device_id }} · {{ channel.channel_id }}</b>
              </el-tooltip>
              <span v-if="multiMode === 'playback'" class="selected-channel-status">{{ multiPlaybackSelectionStatus(channel) }}</span>
            </div>
            <div v-if="multiMode === 'playback'" class="selected-channel-playback">
              <el-date-picker v-model="channel.playback_range" type="datetimerange" range-separator="至"
                start-placeholder="开始时间" end-placeholder="结束时间" :clearable="true"
                :disabled="channel.playback_locked" size="small" />
              <div class="selected-channel-actions">
                <el-button size="small" :disabled="channel.playback_locked || !isValidPlaybackRange(multiDefaultRange)" @click="restoreMultiPlaybackDefault(channel)">恢复默认</el-button>
                <el-button size="small" type="primary" :disabled="channel.playback_locked || !isValidPlaybackRange(channel.playback_range)"
                  @click="confirmMultiPlayback(channel)">确认播放</el-button>
                <el-button v-if="channel.playback_locked && canReplayMultiPlayback(channel)" size="small" type="primary" plain
                  @click="replayMultiPlayback(channel)">重新播放</el-button>
                <el-button v-if="channel.playback_locked" size="small" type="warning" plain
                  @click="stopConfirmedMultiPlayback(channel)">停止并编辑</el-button>
              </div>
            </div>
            <el-button class="selected-channel-remove" type="danger" link @click.stop="removeTreeChannel(channel)">移除</el-button>
          </article>
          <el-empty v-if="!selectedTreeChannels.length" description="暂无已选通道" />
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-12 multi-player-panel" :class="{ 'is-multi-fullscreen': multiFullscreen }">
      <div class="multi-player">
        <GmvMultiGrid ref="multiGridRef" :grid-size="multiGridSize" :cells="multiGridCells" :visible-start="multiVisibleStart" @update:grid-size="handleMultiGridSizeChange"
          @snapshot="handleMultiSnapshot" @snapshot-error="handleMultiSnapshotError" @ptz="handleMultiPtz"
          @output-type-change="handleMultiOutputTypeChange" @playing="handleMultiPlaying"
          @playback-rate-change="handleMultiPlaybackRateChange" @playback-state-change="handleMultiPlaybackStateChange"
          @playback-seek="handleMultiPlaybackSeek" @playback-progress="handleMultiPlaybackProgress"
          @playback-error="handleMultiPlaybackError"
          @playback-switch-cancel="handleMultiPlaybackSwitchCancel"
          @close="handleMultiClose" @reorder="handleMultiReorder">
          <template #summary>
            <div class="multi-player-summary">
              <strong>多画面播放</strong>
              <span>{{ multiPlayerSubtitle }}</span>
            </div>
          </template>
          <template #actions>
            <div class="multi-player-actions">
              <template v-if="multiMode === 'playback'">
                <el-button :loading="multiBulkBusy" :disabled="multiPlaybackStarting || !multiControllableCells.length" @click="toggleAllMultiPlayback">
                  {{ multiPauseActionLabel }}
                </el-button>
                <el-select :model-value="multiDesiredRate" :disabled="multiBulkBusy || multiPlaybackStarting || !multiControllableCells.length"
                  aria-label="统一倍速" class="multi-rate-select" @change="setAllMultiPlaybackRate">
                  <el-option v-for="rate in playbackRates" :key="rate" :label="rate + 'x'" :value="rate" />
                </el-select>
              </template>
              <el-button plain @click="multiFullscreen = !multiFullscreen">{{ multiFullscreen ? '退出满屏' : '满屏' }}</el-button>
            </div>
          </template>
        </GmvMultiGrid>
        <div v-if="multiPageCount > 1" class="multi-pagination">
          <el-pagination v-model:current-page="multiPage" :page-size="multiGridSize" :total="multiCells.length"
            layout="total, prev, pager, next" />
        </div>
      </div>
    </GlassPanel>
  </div>

  <div v-else class="page-grid">
    <GlassPanel class="span-12" title="通道监控" :subtitle="selectedDevice.device_id">
      <div class="monitor-head">
        <div class="device-summary">
          <StatusPill :label="selectedDevice.monitor_status === 1 ? '在线' : '离线'"
            :tone="selectedDevice.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
          <strong>{{ displayDeviceName(selectedDevice) }}</strong>
          <span>Session {{ selectedDevice.session_node_id || '-' }}</span>
        </div>
        <div class="monitor-actions">
          <el-button :loading="resourceLoading" @click="openResourceDrawer">资源能力</el-button>
          <el-tooltip :content="deviceBroadcastReasonText" placement="bottom" :disabled="selectedDevice.monitor_status === 1 && !!availableAudioOutputs.length">
            <el-button v-if="!broadcastSession" type="warning" :loading="broadcastStarting"
              :disabled="!canOperate || selectedDevice.monitor_status !== 1 || !availableAudioOutputs.length" @click="startBroadcast(selectedDevice.device_id)">设备广播</el-button>
          </el-tooltip>
          <el-button v-if="broadcastSession" type="danger" :loading="broadcastStarting" @click="stopBroadcast">停止广播</el-button>
          <el-button :loading="channelLoading" @click="reloadChannels">刷新通道</el-button>
          <el-button type="primary" @click="backToDevices">返回</el-button>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel v-if="showImages" class="span-12" title="抓拍图集" :subtitle="selectedChannelTitle">
      <div class="toolbar">
        <el-button @click="showImages = false">返回通道</el-button>
        <el-button :loading="imageLoading" @click="selectedChannel && loadImages(selectedChannel)">刷新图集</el-button>
      </div>
      <div v-if="images.length" class="image-grid">
        <a v-for="image in images" :key="image.image_id" class="image-card" :href="image.image_url" target="_blank"
          rel="noreferrer">
          <div class="image-preview">
            <img v-if="image.image_url" :src="image.image_url" :alt="image.image_id" />
            <span v-else>暂无图片</span>
          </div>
          <div class="image-meta">
            <b>{{ image.image_id }}</b>
            <span>{{ formatTime(image.created_at_ms) }}</span>
          </div>
        </a>
      </div>
      <el-empty v-else description="暂无抓拍图片" />
    </GlassPanel>

    <template v-else>
      <GlassPanel class="span-12" title="相机列表">
        <div v-loading="channelLoading" class="channel-grid">
          <article v-for="channel in sortedChannels" :key="channel.channel_id" class="channel-card">
            <header class="channel-card-head">
              <div>
                <h2>{{ displayChannelName(channel) }}</h2>
                <p>{{ channel.channel_id }}</p>
              </div>
              <StatusPill :label="channelStatusText(channel)" :tone="channelOnline(channel) ? 'ONLINE' : 'OFFLINE'" />
            </header>
            <div class="channel-tags">
              <span v-if="channelOutputs(channel).length">广播 ×{{ channelOutputs(channel).length }}</span>
              <span v-if="channelOutputs(channel)[0]">{{ capabilitySourceText(channelOutputs(channel)[0]) }}</span>
            </div>
            <button class="channel-cover" type="button" :disabled="!channel.pic_url" @click="previewCover(channel)">
              <img v-if="channel.pic_url" :src="channel.pic_url" :alt="displayChannelName(channel)" />
              <span v-else>暂无封面</span>
            </button>
            <!-- <div class="channel-tags">
              <span>{{ channel.ptz_type || 'PTZ -' }}</span>
              <span>{{ confText(channel.playback_enable, 2, '回放') }}</span>
              <span>{{ confText(channel.snapshot, 2, '抓拍') }}</span>
              <span>{{ confText(channel.biz_enable, 1, '业务') }}</span>
            </div> -->
            <footer class="channel-actions">
              <el-select :model-value="channelOutputType(channel)" class="channel-output-select" aria-label="直播播放方式"
                @change="(value: LiveOutputType) => setChannelOutputType(channel, value)">
                <el-option v-for="option in liveOutputOptions" :key="option.value" :label="option.label" :value="option.value" />
              </el-select>
              <el-button-group class="channel-play-entry live">
                <el-button class="channel-play-main" :disabled="!canPlayLive(channel) || playerRequesting"
                  :loading="isPlayRequesting('preview', channel)" @click="startPlay('preview', channel)">直播</el-button>
                <el-button class="channel-multi-tag" aria-label="加入多画面直播" :disabled="!canPlayLive(channel)"
                  @click="focusChannelInMultiView(channel, 'live')">·多</el-button>
              </el-button-group>
              <el-button-group class="channel-play-entry playback">
                <el-button class="channel-play-main" :disabled="!canPlayback(channel) || playerRequesting"
                  :loading="isPlayRequesting('playback', channel)" @click="requestPlayback(channel)">回放</el-button>
                <el-button class="channel-multi-tag" aria-label="加入多画面回放" :disabled="!canPlayback(channel)"
                  @click="focusChannelInMultiView(channel, 'playback')">·多</el-button>
              </el-button-group>
              <el-button class="channel-second-row" :disabled="!canSnapshot(channel)" :loading="deviceSnapshotLoading[channel.channel_id]"
                @click="requestDeviceSnapshot(channel)">抓拍</el-button>
              <el-button :disabled="!canViewImages(channel)" @click="openImages(channel)">图集</el-button>
              <el-tooltip :content="channelBroadcastReason(channel)" placement="top" :disabled="canBroadcastChannel(channel)">
                <el-button type="warning" :disabled="!canOperate || !canBroadcastChannel(channel) || !!broadcastSession"
                  :loading="broadcastStarting && broadcastScopeId === channel.channel_id"
                  @click="startBroadcast(channel.channel_id)">广播</el-button>
              </el-tooltip>
              <el-button :disabled="!canOperate" @click="openConfig(channel)">配置</el-button>
            </footer>
          </article>
        </div>
        <el-empty v-if="!channelLoading && !sortedChannels.length" description="暂无通道" />
      </GlassPanel>

    </template>

    <el-dialog v-model="coverDialog" title="封面快照" width="720px">
      <img v-if="coverUrl" class="cover-large" :src="coverUrl" alt="封面快照" />
      <el-empty v-else description="暂无封面" />
    </el-dialog>

    <el-dialog v-model="playbackRangeDialog" title="选择历史回放时段" width="560px">
      <el-date-picker v-model="playbackRange" type="datetimerange" range-separator="至"
        start-placeholder="开始时间" end-placeholder="结束时间" :clearable="true" style="width: 100%" />
      <template #footer>
        <el-button @click="playbackRangeDialog = false">取消</el-button>
        <el-button type="primary" :disabled="!playbackRange" @click="confirmPlaybackRange">开始回放</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="playerDialog" :title="playerDialogTitle" width="960px" class="monitor-player-dialog"
      destroy-on-close @close="stopCurrentStream">
      <div v-if="selectedChannel" class="monitor-player">
        <div class="monitor-player-stage">
          <GmvPlayerView ref="singlePlayerRef" :sources="playerSources" :device-id="selectedChannel?.device_id"
            :channel-id="selectedChannel?.channel_id" :title="selectedChannelTitle" :status="playerStatus" :viewers="1"
            :media-mode="lastAction === '历史回放' ? 'playback' : 'live'" :stream-id="lastStream?.stream_id"
            :media-node-id="lastStream?.node_id" :session-node-id="lastStream?.session_node_id"
            :audio-codec="lastStream?.audio_codec"
            :poster="playerPoster" :capabilities="playerCapabilities"
            :controls="playerControls" :playback-duration-ms="playbackDurationMs"
            :playback-start-time-ms="playbackStartTimeMs" :playback-end-time-ms="playbackEndTimeMs"
            :output-type="channelOutputType(selectedChannel)" :output-options="liveOutputOptions"
            :output-switching="singleOutputSwitching" @output-type-change="handleSingleOutputTypeChange"
            :startup-text="singleMediaOperation ? singleStartupText : undefined" :startup-can-cancel="singleCheckpointReached"
            @snapshot="handleSingleSnapshot" @snapshot-error="handleSingleSnapshotError" @ptz="handlePlayerPtz"
            @playing="handleSinglePlaying" @playback-error="handleSinglePlaybackError"
            @playback-switch-cancel="handleSinglePlaybackSwitchCancel"
            @playback-rate-change="handlePlaybackRateChange" @playback-seek="handlePlaybackSeek"
            @playback-state-change="handlePlaybackStateChange" @playback-progress="handlePlaybackProgress" />
          <div v-if="playerRequesting" class="player-loading-badge" role="status" aria-live="polite">
            <span>{{ singleStartupText }}</span>
            <div v-if="singleCheckpointReached" class="player-loading-actions">
              <el-button size="small" type="primary" text @click="acknowledgeSingleWait">继续等待</el-button>
              <el-button size="small" type="danger" text @click="cancelSingleStartup">取消</el-button>
            </div>
          </div>
        </div>
      </div>
      <el-empty v-else description="选择在线通道后播放" />
    </el-dialog>

    <el-drawer v-model="configDrawer" title="相机业务配置" size="420px" class="camera-config-drawer" destroy-on-close>
      <el-form :model="configForm" label-width="110px" class="config-form">
        <el-form-item label="设备ID"><el-input v-model="configForm.device_id" disabled /></el-form-item>
        <el-form-item label="通道ID"><el-input v-model="configForm.channel_id" disabled /></el-form-item>
        <el-form-item label="名称"><el-input v-model="configForm.name" disabled /></el-form-item>
        <el-form-item label="别名"><el-input v-model="configForm.alias_name" maxlength="16" clearable /></el-form-item>
        <el-form-item label="排序"><el-input-number v-model="configForm.sort_no" :min="0" :max="999999" /></el-form-item>
        <el-form-item label="云台控制"><el-select v-model="configForm.ptz_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="语音对讲"><el-select v-model="configForm.talk_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="音频"><el-select v-model="configForm.audio_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="抓拍"><el-select v-model="configForm.snapshot"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="录像"><el-select v-model="configForm.record_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="回放"><el-select v-model="configForm.playback_enable"><el-option
              v-for="option in confOptions" :key="option.value" :label="option.label"
              :value="option.value" /></el-select></el-form-item>
        <el-form-item label="告警"><el-select v-model="configForm.alarm_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="业务启用"><el-select v-model="configForm.biz_enable"><el-option v-for="option in bizOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
      </el-form>
      <template #footer>
        <div class="drawer-footer">
          <el-button @click="configDrawer = false">取消</el-button>
          <el-button type="primary" :loading="configSaving" :disabled="!canOperate" @click="saveConfig">保存</el-button>
        </div>
      </template>
    </el-drawer>

    <el-drawer v-model="resourceDrawer" title="资源识别覆盖管理" size="760px"
      class="resource-capability-drawer" destroy-on-close>
      <div v-loading="resourceLoading" class="resource-capability-content">
        <el-alert title="资源类型优先采用人工覆盖；没有有效覆盖时使用枚举、设备编码和 ParentID 自动识别。" type="info" :closable="false" />
        <el-table :data="resources" max-height="620" empty-text="暂无 Catalog 资源">
          <el-table-column prop="resource_id" label="资源 ID" min-width="190" show-overflow-tooltip />
          <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
          <el-table-column label="编码" width="72"><template #default="{ row }">{{ row.type_code || '-' }}</template></el-table-column>
          <el-table-column label="有效类型" width="120"><template #default="{ row }">{{ resourceKindText(row.effective_kind) }}</template></el-table-column>
          <el-table-column label="来源/状态" width="130"><template #default="{ row }">
            <el-tag :type="classificationTagType(row)">{{ classificationText(row) }}</el-tag>
          </template></el-table-column>
          <el-table-column label="业务所有者" min-width="170" show-overflow-tooltip><template #default="{ row }">{{ row.effective_owner_scope }} · {{ row.effective_owner_id || '-' }}</template></el-table-column>
          <el-table-column label="操作" width="150" fixed="right"><template #default="{ row }">
            <el-button type="primary" link :disabled="!canManageResources" @click="editResource(row)">覆盖</el-button>
            <el-button type="warning" link :disabled="!canManageResources || !row.confirmation || row.confirmation.status !== 1"
              @click="resetResource(row)">恢复自动</el-button>
          </template></el-table-column>
        </el-table>
      </div>
    </el-drawer>

    <el-dialog v-model="resourceEditDialog" title="人工覆盖资源识别" width="520px"
      class="resource-confirm-dialog" destroy-on-close>
      <el-form :model="resourceForm" label-width="110px">
        <el-form-item label="资源 ID"><el-input :model-value="resourceEditing?.resource_id" disabled /></el-form-item>
        <el-form-item label="默认建议"><el-input :model-value="resourceKindText(resourceEditing?.suggested_kind || 'unknown')" disabled /></el-form-item>
        <el-form-item label="资源类型"><el-select v-model="resourceForm.resource_kind" style="width:100%">
          <el-option label="视频资源" value="video" /><el-option label="语音输入" value="audio_input" />
          <el-option label="语音输出" value="audio_output" /><el-option label="其它/否决" value="other" />
        </el-select></el-form-item>
        <el-form-item label="所有者范围"><el-radio-group v-model="resourceForm.owner_scope" @change="syncResourceOwner">
          <el-radio value="device">注册设备</el-radio><el-radio value="resource">Catalog 资源</el-radio>
        </el-radio-group></el-form-item>
        <el-form-item label="业务所有者"><el-select v-if="resourceForm.owner_scope === 'resource'" v-model="resourceForm.owner_id" filterable style="width:100%">
          <el-option v-for="channel in ownerResourceOptions" :key="channel.channel_id" :label="displayChannelName(channel) + ' · ' + channel.channel_id" :value="channel.channel_id" />
        </el-select><el-input v-else v-model="resourceForm.owner_id" disabled /></el-form-item>
        <el-form-item label="说明"><el-input v-model="resourceForm.remark" type="textarea" maxlength="255" show-word-limit /></el-form-item>
      </el-form>
      <template #footer><el-button @click="resourceEditDialog = false">取消</el-button><el-button type="primary" :loading="resourceSaving" @click="saveResource">保存人工覆盖</el-button></template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import { ElMessage, ElMessageBox } from 'element-plus';
import { QuestionFilled } from '@element-plus/icons-vue';
import {
  errorMessage,
  cancelMediaOperation,
  closeStreamOutput,
  continueMediaOperation,
  createStreamOutput,
  getMediaTransport,
  getGbSessionNodeConfig,
  listGbChannelImages,
  listGbChannels,
  listGbDevicePage,
  listGbResources,
  listNodes,
  releaseStream,
  resetGbResourceConfirmation,
  saveGbResourceConfirmation,
  sendGbPtz,
  seekGbPlayback,
  setGbPlaybackSpeed,
  setGbPlaybackState,
  startGbPlayback,
  startGbPreview,
  takeGbSnapshot,
  updateGbChannel,
  type GbChannelImageInfo,
  type GbChannelInfo,
  type GbChannelPayload,
  type GbDeviceInfo,
  type GbPtzPayload,
  type GbResourceInfo,
  type GbSessionConfigInfo,
  type NodeInfo,
  type MediaOperationSummary,
  type StreamSummary,
  type StreamOutputSummary,
} from '@/api/client';
import { startGbMicrophoneBroadcast, type GbBroadcastSession } from '@/audio/gbBroadcast';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { GmvMultiGrid, GmvPlayerView, type GmvCodec, type GmvPlayerControlsConfig, type GmvPtzCommand, type GmvSource, type GmvViewCapabilities } from 'gmv-player';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const singlePlayerRef = ref<InstanceType<typeof GmvPlayerView>>();
const multiGridRef = ref<InstanceType<typeof GmvMultiGrid>>();
type MultiDeviceTreeNode = { expanded: boolean; expand: (callback?: () => void) => void };
type MultiDeviceTreeInstance = {
  getNode: (key: string) => MultiDeviceTreeNode | undefined;
  setCurrentKey: (key: string) => void;
};
const multiDeviceTreeRef = ref<MultiDeviceTreeInstance>();
type LiveOutputType = 'flv' | 'hls' | 'fmp4';
type MultiMode = 'live' | 'playback';
const monitorMode = ref<'devices' | 'multi'>('devices');
const loading = ref(false);
const channelLoading = ref(false);
const resourceLoading = ref(false);
const resourcesLoaded = ref(false);
const imageLoading = ref(false);
const configSaving = ref(false);
const listNodeLoading = ref(false);
const treeLoading = ref(false);
const multiStopping = ref(false);
const deviceName = ref('');
const treeDeviceId = ref('');
const treeDeviceName = ref('');
const devices = ref<GbDeviceInfo[]>([]);
const channels = ref<GbChannelInfo[]>([]);
const resources = ref<GbResourceInfo[]>([]);
const treeDevices = ref<GbDeviceInfo[]>([]);
const images = ref<GbChannelImageInfo[]>([]);
const sessionNodes = ref<NodeInfo[]>([]);
const sessionNodeOptions = ref<SessionNodeOption[]>([]);
const selectedListNodeId = ref('');
const selectedMultiNodeId = ref('');
const page = ref(1);
const pageSize = ref(20);
const total = ref(0);
const treePage = ref(1);
const treePageSize = ref(20);
const treeTotal = ref(0);
const selectedDevice = ref<GbDeviceInfo>();
const selectedChannel = ref<GbChannelInfo>();
const detailDevice = ref<GbDeviceInfo>();
const lastStream = ref<StreamSummary>();
const singleOutput = ref<StreamOutputSummary>();
const singleOutputSwitching = ref(false);
const singlePendingSwitch = ref<{
  previous_type: LiveOutputType;
  previous_output?: StreamOutputSummary;
  previous_endpoint: string;
  next_output: StreamOutputSummary;
}>();
const lastAction = ref('');
const showImages = ref(false);
const deviceDetailDrawer = ref(false);
const coverDialog = ref(false);
const coverUrl = ref('');
const playbackRangeDialog = ref(false);
const playbackRange = ref<[Date, Date]>();
const pendingPlaybackChannel = ref<GbChannelInfo>();
const playbackGeneration = ref(0);
const playbackAnchorPositionSec = ref(0);
const playbackAnchorMediaTimeMs = ref<number>();
const playbackLastMediaTimeMs = ref<number>();
const playbackDisplayedPositionSec = ref(0);
const playbackRangeEnded = ref(false);
let seekInFlight = false;
let queuedSeekMs: number | undefined;
const playerDialog = ref(false);
const playerRequesting = ref(false);
const singleMediaOperation = ref<MediaOperationSummary<unknown>>();
const singleWaitAcknowledged = ref(false);
const pendingPlayKey = ref('');
const configDrawer = ref(false);
const resourceDrawer = ref(false);
const resourceEditDialog = ref(false);
const resourceEditing = ref<GbResourceInfo>();
const resourceSaving = ref(false);
const broadcastStarting = ref(false);
const broadcastSession = ref<GbBroadcastSession>();
const broadcastScopeId = ref('');
const deviceSnapshotLoading = reactive<Record<string, boolean>>({});
const channelOutputTypes = reactive<Record<string, LiveOutputType>>({});
const treeChannelsByDevice = reactive<Record<string, GbChannelInfo[]>>({});
const treeChannelLoading = reactive<Record<string, boolean>>({});
const selectedTreeChannelKeys = ref<string[]>([]);
const selectedTreeChannelItems = ref<SelectedChannelRef[]>([]);
const draggingTreeChannelIndex = ref<number>();
const multiCells = ref<MultiViewCell[]>([]);
const multiGridSize = ref(1);
const multiGridManual = ref(false);
const multiFullscreen = ref(false);
const multiPage = ref(1);
const multiViewLimit = ref(6);
const multiLimitHelpHovered = ref(false);
const multiLimitHelpPinned = ref(false);
const multiMode = ref<MultiMode>('live');
const multiDefaultRange = ref<[Date, Date]>();
const multiPlaybackQueue = ref<string[]>([]);
const multiPlaybackStarting = ref(false);
const multiBulkBusy = ref(false);
const multiDesiredRate = ref(1);
const playbackRates = [0.5, 1, 2, 4];
const multiPlayVersions = reactive<Record<string, number>>({});
const multiStopTasks = new Map<string, Promise<void>>();
const multiPreviewAborts = new Map<string, AbortController>();
const multiOutputAborts = new Map<string, AbortController>();
let stopCurrentStreamTask: Promise<void> | undefined;
let singlePreviewAbort: AbortController | undefined;
let singleOutputAbort: AbortController | undefined;
let playRequestSeq = 0;
let multiViewDisposed = false;
const configForm = reactive<GbChannelPayload & { device_id?: string }>({ channel_id: '', device_id: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const canManageResources = computed(() => auth.session?.role === 'admin');
const resourceForm = reactive({ resource_kind: 'audio_output' as 'video' | 'audio_input' | 'audio_output' | 'other', owner_scope: 'device' as 'device' | 'resource', owner_id: '', remark: '' });

type SessionNodeOption = { node: NodeInfo; config?: GbSessionConfigInfo; disabled: boolean; kindLabel: string; statusLabel: string };
type MultiCellStatus = 'idle' | 'online' | 'playing' | 'paused' | 'queued' | 'stopped' | 'offline' | 'reconnecting' | 'error';
interface MultiViewCell {
  key: string;
  session_node_id: string;
  device_id: string;
  channel_id: string;
  title: string;
  poster?: string;
  stream?: StreamSummary;
  sources: GmvSource[];
  status: MultiCellStatus;
  error?: string;
  operation?: MediaOperationSummary<unknown>;
  channel: GbChannelInfo;
  mode: MultiMode;
  output_type: LiveOutputType;
  playback_start_sec?: number;
  playback_end_sec?: number;
  playback_position_sec?: number;
  playback_generation?: number;
  playback_rate?: number;
  playback_ack_rate?: number;
  playback_state?: 'playing' | 'paused';
  output?: StreamOutputSummary;
  output_switching?: boolean;
  pending_switch?: {
    previous_type: LiveOutputType;
    previous_output?: StreamOutputSummary;
    previous_sources: GmvSource[];
    next_output: StreamOutputSummary;
  };
}
interface SelectedChannelRef {
  session_node_id: string;
  device_id: string;
  channel_id: string;
  title: string;
  poster?: string;
  device_title: string;
  status_text: string;
  channel: GbChannelInfo;
  playback_range?: [Date, Date];
  playback_locked?: boolean;
}
type TreeNodeData =
  | { key: string; label: string; kind: 'device'; device: GbDeviceInfo; leaf: false }
  | { key: string; label: string; kind: 'channel'; channel: GbChannelInfo; leaf: true };
const liveOutputOptions = [
  { value: 'flv', label: 'HTTP-FLV' },
  { value: 'hls', label: 'HLS-fMP4' },
  { value: 'fmp4', label: 'HTTP-fMP4' },
] satisfies Array<{ value: LiveOutputType; label: string }>;

const confOptions = [
  { label: '启用', value: 1 },
  { label: '禁用', value: 0 },
  { label: '设备不支持', value: 2 },
];
const bizOptions = [
  { label: '启用', value: 1 },
  { label: '禁用', value: 0 },
];
const selectedListNodeOption = computed(() => sessionNodeOptions.value.find((item) => item.node.node_id === selectedListNodeId.value));
const selectedMultiNodeOption = computed(() => sessionNodeOptions.value.find((item) => item.node.node_id === selectedMultiNodeId.value));
const selectedMultiNodeLabel = computed(() => selectedMultiNodeOption.value ? listNodeLabel(selectedMultiNodeOption.value) : '未选择信令节点');
const videoResourceIds = computed(() => new Set(resources.value.filter((resource) => resource.effective_kind === 'video').map((resource) => resource.resource_id)));
const sortedChannels = computed(() => channels.value.filter((channel) => !resourcesLoaded.value || videoResourceIds.value.has(channel.channel_id)).sort((left, right) => {
  const sortNo = Number(left.sort_no || 0) - Number(right.sort_no || 0);
  return sortNo || displayChannelName(left).localeCompare(displayChannelName(right), 'zh-Hans-CN');
}));
const audioOutputs = computed(() => resources.value.filter((resource) => resource.effective_kind === 'audio_output'));
const availableAudioOutputs = computed(() => audioOutputs.value.filter((resource) => resource.available));
const deviceBroadcastReason = computed(() => selectedDevice.value?.monitor_status !== 1 ? 'DEVICE_OFFLINE' : availableAudioOutputs.value.length ? '' : audioOutputs.value[0]?.unavailable_reason || 'NO_AUDIO_OUTPUT');
const deviceBroadcastReasonText = computed(() => broadcastReasonText(deviceBroadcastReason.value));
const ownerResourceOptions = computed(() => channels.value.filter((channel) => channel.channel_id !== resourceEditing.value?.resource_id));
const selectedTreeChannels = computed<SelectedChannelRef[]>(() => selectedTreeChannelItems.value);
const multiLimitHelpVisible = computed(() => multiLimitHelpHovered.value || multiLimitHelpPinned.value);
const treeProps = { label: 'label', isLeaf: 'leaf' };
const treeDeviceNodes = computed<TreeNodeData[]>(() => treeDevices.value.map((device) => ({
  key: multiDeviceKey(device.session_node_id || selectedMultiNodeId.value, device.device_id),
  label: displayDeviceName(device),
  kind: 'device',
  device,
  leaf: false,
})));
const selectedChannelTitle = computed(() => selectedChannel.value ? displayChannelName(selectedChannel.value) : '未选择通道');
const deviceDetailTitle = computed(() => detailDevice.value ? '设备详情 · ' + displayDeviceName(detailDevice.value) : '设备详情');
const playerDialogTitle = computed(() => lastAction.value ? lastAction.value + ' · ' + selectedChannelTitle.value : '播放窗口');
const singleCheckpointReached = computed(() => {
  const operation = singleMediaOperation.value;
  return !!operation && operation.checkpoint_ms > 0 && operation.elapsed_ms >= operation.checkpoint_ms;
});
const singleStartupText = computed(() => mediaOperationText(singleMediaOperation.value, singleWaitAcknowledged.value));
const multiPlayerSubtitle = computed(() => {
  const label = multiMode.value === 'live' ? '实时直播' : '历史回放';
  return multiCells.value.length
    ? `${label} · 运行中 ${multiCells.value.filter((cell) => cell.stream?.state === 'running').length} 路`
    : `${label} · 选择通道后播放`;
});
const multiPageCount = computed(() => Math.max(1, Math.ceil(multiCells.value.length / multiGridSize.value)));
const multiVisibleStart = computed(() => (multiPage.value - 1) * multiGridSize.value);
const multiControllableCells = computed(() => multiCells.value.filter((cell) => cell.mode === 'playback' && !!cell.stream?.stream_id));
const multiPauseActionLabel = computed(() => multiControllableCells.value.some((cell) => cell.playback_state !== 'paused') ? '统一暂停' : '统一继续');
const playerStatus = computed(() => lastStream.value?.state === 'running' ? 'playing' : selectedChannel.value && channelOnline(selectedChannel.value) ? 'online' : 'idle');
const playerPoster = computed(() => selectedChannel.value?.pic_url || undefined);
const playerCapabilities = computed<GmvViewCapabilities>(() => {
  const channel = selectedChannel.value;
  const hasAudio = streamHasAudio(lastStream.value);
  return {
    ptz: channel ? canPtz(channel) : false,
    presets: false,
    snapshot: true,
    record: false,
    playback: channel ? lastAction.value === '历史回放' && canPlayback(channel) : false,
    audio: channel ? canAudio(channel) && hasAudio : false,
    talk: false,
    streamSwitch: false,
    aiOverlay: false,
  };
});
const playerControls = computed<GmvPlayerControlsConfig>(() => {
  const channel = selectedChannel.value;
  const playback = lastAction.value === '历史回放';
  const items: GmvPlayerControlsConfig['items'] = ['play', 'snapshot', 'fullscreen'];
  if (playback && channel && canPlayback(channel)) items.push('timeline');
  const overflowItems: GmvPlayerControlsConfig['items'] = [];
  if (!playback) overflowItems.push('outputType');
  overflowItems.push('info');
  if (channel && canAudio(channel)) overflowItems.push('audio');
  if (!playback && channel && canPtz(channel)) overflowItems.push('ptz');
  if (playback && channel && canPlayback(channel)) overflowItems.push('playbackRate');
  return { items, overflowItems, visibility: 'auto', autoHideDelayMs: 3000, playbackRates: [0.5, 1, 2, 4] };
});
const playbackDurationMs = computed(() => {
  const stream = lastStream.value;
  if (!stream?.playback_start_time_sec || !stream.playback_end_time_sec) return 86_400_000;
  return Math.max(1_000, (stream.playback_end_time_sec - stream.playback_start_time_sec) * 1_000);
});
const playbackStartTimeMs = computed(() => {
  const startSec = lastStream.value?.playback_start_time_sec;
  return startSec ? startSec * 1_000 : undefined;
});
const playbackEndTimeMs = computed(() => {
  const endSec = lastStream.value?.playback_end_time_sec;
  return endSec ? endSec * 1_000 : undefined;
});
const playerSources = computed<GmvSource[]>(() => {
  const endpoint = lastStream.value?.endpoint;
  if (!endpoint) return [];
  const protocol = streamProtocol(endpoint);
  const codec = streamCodec(lastStream.value);
  const hasAudio = streamHasAudio(lastStream.value);
  return [{
    protocol,
    codec,
    url: endpoint,
    mimeCodec: lastStream.value?.mime_codec ?? fmp4MimeCodec(codec, hasAudio),
    hasAudio,
    rateMode: protocol === 'mp4' ? 'local-file' : lastAction.value === '历史回放' ? 'remote-stream' : 'disabled',
    label: streamSourceLabel(codec, hasAudio),
    priority: 1,
  }];
});
const multiGridCells = computed(() => multiCells.value.map((cell) => {
  const capabilities = multiCellCapabilities(cell);
  return {
    cellId: cell.key,
    sources: cell.sources,
    title: cell.error ? cell.title + ' · ' + cell.error : cell.title,
    deviceId: cell.device_id,
    channelId: cell.channel_id,
    status: multiPlayerDeviceStatus(cell.status),
    viewers: 1,
    mediaMode: cell.mode,
    streamId: cell.stream?.stream_id,
    mediaNodeId: cell.stream?.node_id,
    sessionNodeId: cell.session_node_id,
    audioCodec: cell.stream?.audio_codec,
    poster: cell.poster,
    capabilities,
    controls: multiCellControls(capabilities),
    outputType: cell.mode === 'live' ? cell.output_type : undefined,
    outputOptions: cell.mode === 'live' ? liveOutputOptions : undefined,
    outputSwitching: cell.mode === 'live' ? cell.output_switching : undefined,
    playbackDurationMs: playbackCellDurationMs(cell),
    playbackStartTimeMs: cell.mode === 'playback' && cell.playback_start_sec ? cell.playback_start_sec * 1_000 : undefined,
    playbackEndTimeMs: cell.mode === 'playback' && cell.playback_end_sec ? cell.playback_end_sec * 1_000 : undefined,
    startupText: cell.operation ? mediaOperationText(cell.operation) : undefined,
    startupCanCancel: !!cell.operation && cell.operation.checkpoint_ms > 0 && cell.operation.elapsed_ms >= cell.operation.checkpoint_ms,
  };
}));

watch(() => multiCells.value.length, () => {
  applyAutoMultiGridSize();
  clampMultiPage();
});
watch(multiGridSize, clampMultiPage);

function clampMultiPage() {
  if (multiPage.value > multiPageCount.value) multiPage.value = multiPageCount.value;
  if (multiPage.value < 1) multiPage.value = 1;
}

function displayDeviceName(device: GbDeviceInfo) { return device.alias || device.device_id; }
function displayChannelName(channel: GbChannelInfo) { return channel.alias_name || channel.name || channel.channel_id; }
function channelOutputKey(channel: GbChannelInfo) { return `${channel.device_id}:${channel.channel_id}`; }
function channelOutputType(channel: GbChannelInfo): LiveOutputType { return channelOutputTypes[channelOutputKey(channel)] ?? 'flv'; }
function setChannelOutputType(channel: GbChannelInfo, outputType: LiveOutputType) { channelOutputTypes[channelOutputKey(channel)] = outputType; }
function selectedChannelTooltip(channel: SelectedChannelRef) { return `${channel.session_node_id} · ${channel.device_title} · ${channel.title}`; }
function autoMultiGridSize(count: number) {
  if (count <= 1) return 1;
  if (count <= 4) return 4;
  if (count <= 9) return 9;
  return 16;
}
function applyAutoMultiGridSize() {
  if (!multiGridManual.value) multiGridSize.value = autoMultiGridSize(multiCells.value.length);
}
function handleMultiGridSizeChange(value: number) {
  multiGridManual.value = true;
  multiGridSize.value = multiViewLimit.value <= 6 && value > 9 ? 9 : value;
}
function tableIndex(index: number) { return (page.value - 1) * pageSize.value + index + 1; }
function emptyText(value: unknown) { return value === undefined || value === null || value === '' ? '-' : String(value); }
function countText(value: unknown) { const count = Number(value || 0); return count > 0 ? String(count) : '-'; }
function normalizeKind(value?: string | null) { return (value || '').trim().toLowerCase(); }
function nodeKindLabel(node: NodeInfo) { return (node.kind || node.service || node.config?.service || 'node').toUpperCase(); }
function nodeStatusLabel(disabled: boolean, reason?: string) { return reason || (disabled ? '离线' : '在线'); }
function buildSessionNodeOption(node: NodeInfo, config?: GbSessionConfigInfo, disabledReason?: string): SessionNodeOption {
  const disabled = !isNodeOnline(node) || !config?.domain_id;
  const reason = disabledReason || (isNodeOnline(node) && !config?.domain_id ? '缺少 domain 配置' : undefined);
  return { node, config, disabled, kindLabel: nodeKindLabel(node), statusLabel: nodeStatusLabel(disabled, reason) };
}
function isGbSessionNode(node: NodeInfo) { return normalizeKind(node.kind) === 'session-gb28181' || normalizeKind(node.service) === 'session-gb28181' || normalizeKind(node.protocol) === 'gb28181'; }
function isNodeOnline(node?: NodeInfo) { return !!node && node.connection === 'CONNECTED' && node.scheduling === 'ENABLED'; }
function listNodeLabel(option: SessionNodeOption) { return `${option.kindLabel} · ${option.node.node_id} · ${option.statusLabel}`; }
function confValue(value: unknown, defaultValue = 2) { return value === undefined || value === null ? defaultValue : Number(value); }
function confEnabled(value: unknown) { return confValue(value) === 1; }
function bizEnabled(channel: GbChannelInfo) { return confValue(channel.biz_enable, 1) === 1; }
function channelOnline(channel: GbChannelInfo) { return ['ON', 'ONLINE'].includes((channel.status || '').toUpperCase()); }
function channelStatusText(channel: GbChannelInfo) { return channelOnline(channel) ? '在线' : '离线'; }
function confText(value: unknown, defaultValue: number, label: string) {
  const v = confValue(value, defaultValue);
  if (v === 1) return label + '启用';
  if (v === 0) return label + '禁用';
  return label + '不支持';
}
function canPlayLive(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel); }
function canPlayback(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.playback_enable); }
function canSelectMultiChannel(channel: GbChannelInfo) { return multiMode.value === 'live' ? canPlayLive(channel) : canPlayback(channel); }
function canSnapshot(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.snapshot); }
function canPtz(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.ptz_enable); }
function canAudio(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.audio_enable); }
function canViewImages(channel: GbChannelInfo) { return bizEnabled(channel); }
function multiCellCapabilities(cell: MultiViewCell): GmvViewCapabilities {
  const hasAudio = cell.sources.some((source) => source.hasAudio);
  return {
    ptz: cell.mode === 'live' && canPtz(cell.channel),
    presets: false,
    snapshot: true,
    record: false,
    playback: cell.mode === 'playback' && canPlayback(cell.channel),
    audio: hasAudio && canAudio(cell.channel),
    talk: false,
    streamSwitch: cell.sources.length > 1,
    aiOverlay: false,
  };
}
function multiCellControls(capabilities: GmvViewCapabilities): GmvPlayerControlsConfig {
  const items: GmvPlayerControlsConfig['items'] = ['play', 'snapshot', 'fullscreen'];
  if (capabilities.playback) items.push('timeline');
  const overflowItems: GmvPlayerControlsConfig['items'] = ['info'];
  if (!capabilities.playback) overflowItems.unshift('outputType');
  if (capabilities.audio) overflowItems.push('audio');
  if (capabilities.ptz) overflowItems.push('ptz');
  if (capabilities.streamSwitch) overflowItems.push('streamSwitch');
  if (capabilities.playback) overflowItems.push('playbackRate');
  return { items, overflowItems, visibility: 'auto', autoHideDelayMs: 3000, playbackRates };
}
function playRequestKey(kind: 'preview' | 'playback', channel: GbChannelInfo) { return `${kind}:${channel.device_id}:${channel.channel_id}`; }
function isPlayRequesting(kind: 'preview' | 'playback', channel: GbChannelInfo) { return playerRequesting.value && pendingPlayKey.value === playRequestKey(kind, channel); }
function streamProtocol(endpoint: string): GmvSource['protocol'] {
  const path = endpoint.split('?')[0].toLowerCase();
  if (path.endsWith('.fmp4')) return 'fmp4';
  if (path.endsWith('.m3u8')) return 'hls';
  if (path.endsWith('.mp4')) return 'mp4';
  return 'flv';
}
function streamCodec(stream?: StreamSummary): GmvCodec | undefined {
  const codec = (stream?.video_codec || '').trim().toLowerCase();
  if (codec === 'h264' || codec === 'h.264' || codec === 'avc' || codec === 'avc1') return 'h264';
  if (codec === 'h265' || codec === 'h.265' || codec === 'hevc' || codec === 'hev1' || codec === 'hvc1') return 'h265';
  return undefined;
}
function streamAudioCodec(stream?: StreamSummary) {
  const codec = (stream?.audio_codec || '').trim().toLowerCase();
  return codec && !['none', 'unknown', 'null'].includes(codec) ? codec : undefined;
}
function streamHasAudio(stream?: StreamSummary) { return !!streamAudioCodec(stream); }
function fmp4MimeCodec(codec?: GmvCodec, hasAudio = false) {
  const audioCodec = hasAudio ? ', mp4a.40.2' : '';
  if (codec === 'h264') return `video/mp4; codecs="avc1.42E01E${audioCodec}"`;
  if (codec === 'h265') return `video/mp4; codecs="hvc1.1.6.L123.B0${audioCodec}"`;
  return undefined;
}
function streamSourceLabel(codec: GmvCodec | undefined, hasAudio: boolean) {
  return `默认${hasAudio ? '音视频' : '静音'} · ${codec?.toUpperCase() || 'AUTO'}`;
}
function formatTime(value: number) {
  if (!value) return '-';
  return new Date(value).toLocaleString();
}
function resourceKindText(kind: string) {
  return ({ video: '视频资源', audio_input: '语音输入', audio_output: '语音输出', other: '其它', unknown: '未知' } as Record<string, string>)[kind] || kind || '未知';
}
function classificationText(resource: GbResourceInfo) {
  return ({ default: '自动', manual: '人工', manual_stale: '人工·待复核', unknown: '未知', conflict: '冲突', orphan: '孤儿' } as Record<string, string>)[resource.classification_mode] || resource.classification_mode;
}
function classificationTagType(resource: GbResourceInfo): 'success' | 'warning' | 'danger' | 'info' {
  if (resource.classification_mode === 'manual') return 'success';
  if (resource.classification_mode === 'default') return 'info';
  if (resource.classification_mode === 'manual_stale') return 'warning';
  return 'danger';
}
function capabilitySourceText(resource: GbResourceInfo) {
  return resource.classification_mode.startsWith('manual') ? '人工' : `自动${resource.type_code ? `（编码 ${resource.type_code}）` : ''}`;
}
function channelOutputs(channel: GbChannelInfo) {
  return audioOutputs.value.filter((resource) => resource.resource_id === channel.channel_id || resource.effective_owner_id === channel.channel_id);
}
function canBroadcastChannel(channel: GbChannelInfo) { return selectedDevice.value?.monitor_status === 1 && channelOutputs(channel).some((resource) => resource.available); }
function channelBroadcastReason(channel: GbChannelInfo) {
  if (selectedDevice.value?.monitor_status !== 1) return broadcastReasonText('DEVICE_OFFLINE');
  const outputs = channelOutputs(channel);
  return broadcastReasonText(outputs.find((resource) => !resource.available)?.unavailable_reason || (outputs.length ? '' : 'NO_AUDIO_OUTPUT'));
}
function broadcastReasonText(reason: string) {
  return ({ NO_AUDIO_OUTPUT: '没有语音输出资源', UNKNOWN_RESOURCE_KIND: '资源类型未知', RESOURCE_CONFLICT: '资源归属冲突', RESOURCE_ORPHAN: '资源已不在 Catalog', DEVICE_OFFLINE: '设备离线', OUTPUT_OFFLINE: '语音输出离线', BUSINESS_DISABLED: '业务已禁用' } as Record<string, string>)[reason] || reason || '可广播';
}
function multiChannelKey(sessionNodeId: string, deviceId: string, channelId: string) {
  return `${sessionNodeId}:${deviceId}:${channelId}`;
}
function channelKey(channel: GbChannelInfo) {
  return multiChannelKey(selectedMultiNodeId.value, channel.device_id, channel.channel_id);
}
function multiDeviceKey(sessionNodeId: string, deviceId: string) {
  return `${sessionNodeId}:${deviceId}`;
}
function streamSources(stream?: StreamSummary, mode: MultiMode = 'live'): GmvSource[] {
  const endpoint = stream?.endpoint;
  if (!endpoint) return [];
  const protocol = streamProtocol(endpoint);
  const codec = streamCodec(stream);
  const hasAudio = streamHasAudio(stream);
  return [{
    protocol,
    codec,
    url: endpoint,
    mimeCodec: stream?.mime_codec ?? fmp4MimeCodec(codec, hasAudio),
    hasAudio,
    rateMode: protocol === 'mp4' ? 'local-file' : mode === 'playback' ? 'remote-stream' : 'disabled',
    label: streamSourceLabel(codec, hasAudio),
    priority: 1,
  }];
}
function playbackCellDurationMs(cell: MultiViewCell) {
  if (cell.mode !== 'playback' || !cell.playback_start_sec || !cell.playback_end_sec) return undefined;
  return Math.max(1_000, (cell.playback_end_sec - cell.playback_start_sec) * 1_000);
}
function multiPlayerDeviceStatus(status: MultiCellStatus): 'online' | 'offline' | 'playing' | 'reconnecting' | 'error' | 'idle' {
  if (status === 'paused') return 'online';
  if (status === 'queued') return 'reconnecting';
  if (status === 'stopped') return 'idle';
  return status;
}
function isValidPlaybackRange(range?: [Date, Date]): range is [Date, Date] {
  return !!range && range[0] instanceof Date && range[1] instanceof Date && range[0].getTime() < range[1].getTime();
}
function clearTreeLoadedChannelState() {
  for (const key of Object.keys(treeChannelsByDevice)) delete treeChannelsByDevice[key];
  for (const key of Object.keys(treeChannelLoading)) delete treeChannelLoading[key];
}
function clearTreeChannelState() {
  clearTreeLoadedChannelState();
  selectedTreeChannelKeys.value = [];
  selectedTreeChannelItems.value = [];
}
function clearTreeDeviceBrowserState() {
  treeDeviceId.value = '';
  treeDeviceName.value = '';
  treeDevices.value = [];
  treePage.value = 1;
  treeTotal.value = 0;
  clearTreeLoadedChannelState();
}
function clearTreeDeviceState() {
  clearTreeDeviceBrowserState();
  clearTreeChannelState();
}
async function openMultiView(mode: MultiMode = 'live') {
  multiMode.value = mode;
  multiDefaultRange.value = undefined;
  monitorMode.value = 'multi';
  multiGridManual.value = false;
  applyAutoMultiGridSize();
  selectedDevice.value = undefined;
  showImages.value = false;
  await Promise.all([loadMultiViewCapability(), loadSessionNodes()]);
}
async function handleMultiModeChange(value: string | number | boolean | undefined) {
  const nextMode = value === 'playback' ? 'playback' : 'live';
  if (nextMode === multiMode.value) return;
  const previous = [...selectedTreeChannelItems.value];
  if (previous.length) {
    try {
      const message = nextMode === 'playback'
        ? '切换到历史回放将停止当前全部画面，切换后请先设置默认时段。'
        : '切换模式将停止当前全部画面，并保留目标模式仍可用的通道。';
      await ElMessageBox.confirm(message, '切换多画面模式', {
        confirmButtonText: '确认切换', cancelButtonText: '取消', type: 'warning',
      });
    } catch {
      return;
    }
  }
  await stopAllMultiStreams({ quiet: true });
  multiMode.value = nextMode;
  const retained = previous.filter((item) => nextMode === 'live'
    ? canPlayLive(item.channel)
    : canPlayback(item.channel) && isValidPlaybackRange(multiDefaultRange.value));
  for (const item of retained) {
    const range = nextMode === 'playback' && isValidPlaybackRange(multiDefaultRange.value)
      ? [new Date(multiDefaultRange.value[0]), new Date(multiDefaultRange.value[1])] as [Date, Date]
      : undefined;
    selectedTreeChannelKeys.value.push(selectedChannelKey(item));
    const selected = { ...item, playback_range: range, playback_locked: false };
    selectedTreeChannelItems.value.push(selected);
    await startSelectedMultiChannel(selected);
  }
  if (retained.length !== previous.length) ElMessage.info(`已移除 ${previous.length - retained.length} 路不满足目标模式能力的通道`);
}
async function loadMultiViewCapability() {
  try {
    const capability = await getMediaTransport();
    const canUseSixteenViews = window.location.protocol === 'https:'
      && capability.scheme === 'https'
      && capability.http_version === 'h2';
    multiViewLimit.value = canUseSixteenViews ? Math.min(16, Math.max(1, capability.multi_view_limit)) : 6;
  } catch {
    multiViewLimit.value = 6;
  }
  if (multiGridSize.value > 9 && multiViewLimit.value <= 6) multiGridSize.value = 9;
}
async function backToDeviceListFromMulti() {
  await stopAllMultiStreams();
  multiFullscreen.value = false;
  monitorMode.value = 'devices';
  selectedMultiNodeId.value = '';
  clearTreeDeviceState();
  await loadDevices();
}
async function selectMultiNode(nodeId: string) {
  selectedMultiNodeId.value = nodeId;
  clearTreeDeviceBrowserState();
  await queryTreeDevices();
}
async function searchTreeDevices() {
  treePage.value = 1;
  await queryTreeDevices();
}
async function queryTreeDevices() {
  const option = selectedMultiNodeOption.value;
  if (!option || option.disabled || !option.config?.domain_id) {
    treeDevices.value = [];
    treeTotal.value = 0;
    return;
  }
  treeLoading.value = true;
  try {
    clearTreeLoadedChannelState();
    const result = await listGbDevicePage(
      treePage.value,
      treePageSize.value,
      option.node.node_id,
      option.config.domain_id,
      treeDeviceId.value,
      treeDeviceName.value,
      true,
    );
    treeDevices.value = result.items;
    treeTotal.value = result.total;
    treePage.value = result.page;
    treePageSize.value = result.page_size;
  } catch (error) {
    ElMessage.error(errorMessage(error, '设备查询失败'));
  } finally {
    treeLoading.value = false;
  }
}
async function resetTreeDevices() {
  await stopAllMultiStreams({ quiet: true });
  clearTreeDeviceState();
  if (selectedMultiNodeOption.value) await queryTreeDevices();
}
async function handleTreePageSizeChange() {
  treePage.value = 1;
  await queryTreeDevices();
}
async function loadTreeDeviceChannels(device: GbDeviceInfo) {
  if (treeChannelsByDevice[device.device_id]) return treeChannelsByDevice[device.device_id];
  treeChannelLoading[device.device_id] = true;
  try {
    const [channelRows, resourceRows] = await Promise.all([
      listGbChannels(device.device_id, device.session_node_id || selectedMultiNodeId.value),
      listGbResources(device.device_id, device.session_node_id || selectedMultiNodeId.value),
    ]);
    const videoIds = new Set(resourceRows.filter((resource) => resource.effective_kind === 'video').map((resource) => resource.resource_id));
    treeChannelsByDevice[device.device_id] = resourceRows.length
      ? channelRows.filter((channel) => videoIds.has(channel.channel_id))
      : channelRows;
  } catch (error) {
    treeChannelsByDevice[device.device_id] = [];
    ElMessage.error(errorMessage(error, '通道加载失败'));
  } finally {
    treeChannelLoading[device.device_id] = false;
  }
  return treeChannelsByDevice[device.device_id];
}
async function loadTreeNode(node: { level: number; data?: TreeNodeData }, resolve: (data: TreeNodeData[]) => void) {
  const data = node.data;
  if (!data || data.kind !== 'device') {
    resolve([]);
    return;
  }
  const channels = await loadTreeDeviceChannels(data.device);
  resolve(channels.map((channel) => ({
    key: channelKey(channel),
    label: displayChannelName(channel),
    kind: 'channel',
    channel,
    leaf: true,
  })));
}
async function toggleTreeChannel(channel: GbChannelInfo, checked: boolean) {
  const key = channelKey(channel);
  if (checked) {
    if (selectedTreeChannelKeys.value.includes(key)) return;
    if (multiMode.value === 'playback' && !isValidPlaybackRange(multiDefaultRange.value)) {
      ElMessage.warning('请先选择有效的默认回放时段');
      return;
    }
    if (selectedTreeChannelKeys.value.length >= multiViewLimit.value) {
      ElMessage.warning(`当前传输能力最多选择 ${multiViewLimit.value} 个通道`);
      return;
    }
    if (multiMode.value === 'playback') {
      const sameDeviceCount = selectedTreeChannelItems.value.filter((item) => (
        item.session_node_id === selectedMultiNodeId.value && item.device_id === channel.device_id
      )).length;
      if (sameDeviceCount >= 4) {
        try {
          await ElMessageBox.confirm(
            `同一设备 ${channel.device_id} 即将加入第 ${sameDeviceCount + 1} 路回放，边端设备可能无法同时承载，请确认添加。`,
            '边端资源提示',
            { confirmButtonText: '确认添加', cancelButtonText: '取消', type: 'warning' },
          );
        } catch {
          return;
        }
      }
    }
    const device = treeDevices.value.find((item) => item.device_id === channel.device_id);
    const defaultRange = isValidPlaybackRange(multiDefaultRange.value)
      ? [new Date(multiDefaultRange.value[0]), new Date(multiDefaultRange.value[1])] as [Date, Date]
      : undefined;
    selectedTreeChannelKeys.value.push(key);
    selectedTreeChannelItems.value.push({
      session_node_id: selectedMultiNodeId.value,
      device_id: channel.device_id,
      channel_id: channel.channel_id,
      title: displayChannelName(channel),
      poster: channel.pic_url || undefined,
      device_title: device ? displayDeviceName(device) : channel.device_id,
      status_text: channelStatusText(channel),
      channel,
      playback_range: multiMode.value === 'playback' ? defaultRange : undefined,
      playback_locked: false,
    });
    await startSelectedMultiChannel(selectedTreeChannelItems.value[selectedTreeChannelItems.value.length - 1]);
    return;
  }
  await stopMultiCell(key);
}
async function removeTreeChannel(channel: SelectedChannelRef) {
  await stopMultiCell(selectedChannelKey(channel));
}
function restoreMultiPlaybackDefault(channel: SelectedChannelRef) {
  if (channel.playback_locked) return;
  channel.playback_range = isValidPlaybackRange(multiDefaultRange.value)
    ? [new Date(multiDefaultRange.value[0]), new Date(multiDefaultRange.value[1])]
    : undefined;
}
function multiPlaybackSelectionStatus(channel: SelectedChannelRef) {
  const cell = multiCells.value.find((item) => item.key === selectedChannelKey(channel));
  if (!channel.playback_locked) {
    const usingDefault = isValidPlaybackRange(channel.playback_range)
      && isValidPlaybackRange(multiDefaultRange.value)
      && channel.playback_range[0].getTime() === multiDefaultRange.value[0].getTime()
      && channel.playback_range[1].getTime() === multiDefaultRange.value[1].getTime();
    return usingDefault ? '默认时段 · 可编辑' : '自定义时段 · 可编辑';
  }
  const labels: Record<MultiCellStatus, string> = {
    idle: '待确认', online: '在线', playing: '播放中', paused: '已暂停', queued: '排队中',
    stopped: '已停止', offline: '离线', reconnecting: '正在启动', error: '播放失败',
  };
  return cell ? labels[cell.status] : '已确认';
}
async function confirmMultiPlayback(channel: SelectedChannelRef) {
  if (channel.playback_locked || !isValidPlaybackRange(channel.playback_range)) return;
  const key = selectedChannelKey(channel);
  const cell = multiCells.value.find((item) => item.key === key);
  if (!cell) return;
  const startSec = Math.floor(channel.playback_range[0].getTime() / 1_000);
  const endSec = Math.floor(channel.playback_range[1].getTime() / 1_000);
  channel.playback_locked = true;
  upsertMultiCell({
    ...cell,
    mode: 'playback',
    playback_start_sec: startSec,
    playback_end_sec: endSec,
    playback_position_sec: startSec,
    playback_generation: 0,
    playback_rate: multiDesiredRate.value,
    playback_ack_rate: 1,
    playback_state: 'playing',
    status: 'queued',
    error: undefined,
  });
  enqueueMultiPlayback(key);
}
async function stopConfirmedMultiPlayback(channel: SelectedChannelRef) {
  const key = selectedChannelKey(channel);
  const cell = multiCells.value.find((item) => item.key === key);
  if (!cell) return;
  bumpMultiPlayVersion(key);
  multiPlaybackQueue.value = multiPlaybackQueue.value.filter((item) => item !== key);
  multiPreviewAborts.get(key)?.abort();
  multiPreviewAborts.delete(key);
  await disposeMultiCellMedia(cell);
  channel.playback_locked = false;
  upsertMultiCell({
    ...cell,
    stream: undefined,
    sources: [],
    operation: undefined,
    playback_generation: 0,
    playback_position_sec: cell.playback_start_sec,
    playback_ack_rate: 1,
    playback_state: undefined,
    status: 'idle',
    error: undefined,
  });
}
function canReplayMultiPlayback(channel: SelectedChannelRef) {
  const status = multiCells.value.find((item) => item.key === selectedChannelKey(channel))?.status;
  return status === 'error' || status === 'stopped';
}
async function replayMultiPlayback(channel: SelectedChannelRef) {
  if (!channel.playback_locked || !isValidPlaybackRange(channel.playback_range)) return;
  const key = selectedChannelKey(channel);
  const cell = multiCells.value.find((item) => item.key === key);
  if (!cell || !canReplayMultiPlayback(channel)) return;
  bumpMultiPlayVersion(key);
  await disposeMultiCellMedia(cell);
  upsertMultiCell({
    ...cell,
    stream: undefined,
    sources: [],
    operation: undefined,
    playback_position_sec: cell.playback_start_sec,
    playback_generation: 0,
    playback_ack_rate: 1,
    playback_state: 'playing',
    status: 'queued',
    error: undefined,
  });
  enqueueMultiPlayback(key);
}
function syncSelectedTreeChannelKeys() {
  selectedTreeChannelKeys.value = selectedTreeChannelItems.value.map(selectedChannelKey);
}
function syncSelectedTreeChannelsOrderFromCells() {
  const order = new Map(multiCells.value.map((cell, index) => [cell.key, index]));
  selectedTreeChannelItems.value = [...selectedTreeChannelItems.value].sort((left, right) => {
    const leftOrder = order.get(selectedChannelKey(left)) ?? Number.MAX_SAFE_INTEGER;
    const rightOrder = order.get(selectedChannelKey(right)) ?? Number.MAX_SAFE_INTEGER;
    return leftOrder - rightOrder;
  });
  syncSelectedTreeChannelKeys();
}
function syncMultiCellsOrder() {
  const order = new Map(selectedTreeChannelItems.value.map((item, index) => [selectedChannelKey(item), index]));
  multiCells.value = [...multiCells.value].sort((left, right) => {
    const leftOrder = order.get(left.key) ?? Number.MAX_SAFE_INTEGER;
    const rightOrder = order.get(right.key) ?? Number.MAX_SAFE_INTEGER;
    return leftOrder - rightOrder;
  });
}
function handleSelectedChannelDragStart(index: number) {
  draggingTreeChannelIndex.value = index;
}
function handleSelectedChannelDrop(targetIndex: number) {
  const sourceIndex = draggingTreeChannelIndex.value;
  draggingTreeChannelIndex.value = undefined;
  if (sourceIndex === undefined || sourceIndex === targetIndex) return;
  const items = [...selectedTreeChannelItems.value];
  const [item] = items.splice(sourceIndex, 1);
  if (!item) return;
  items.splice(targetIndex, 0, item);
  selectedTreeChannelItems.value = items;
  syncSelectedTreeChannelKeys();
  syncMultiCellsOrder();
}
function handleSelectedChannelDragEnd() {
  draggingTreeChannelIndex.value = undefined;
}
function bumpMultiPlayVersion(key: string) {
  multiPlayVersions[key] = (multiPlayVersions[key] || 0) + 1;
  return multiPlayVersions[key];
}
function selectedChannelKey(channel: SelectedChannelRef) {
  return multiChannelKey(channel.session_node_id, channel.device_id, channel.channel_id);
}
function upsertMultiCell(cell: MultiViewCell) {
  const index = multiCells.value.findIndex((item) => item.key === cell.key);
  if (index >= 0) {
    multiCells.value.splice(index, 1, cell);
  } else {
    multiCells.value.push(cell);
    applyAutoMultiGridSize();
    multiPage.value = Math.ceil(multiCells.value.length / multiGridSize.value);
  }
  syncMultiCellsOrder();
}
async function startSelectedMultiChannel(channel?: SelectedChannelRef) {
  if (!channel) return;
  const key = selectedChannelKey(channel);
  upsertMultiCell({
    key,
    session_node_id: channel.session_node_id,
    device_id: channel.device_id,
    channel_id: channel.channel_id,
    title: channel.title,
    poster: channel.poster,
    sources: [],
    status: multiMode.value === 'live' ? 'reconnecting' : 'idle',
    channel: channel.channel,
    mode: multiMode.value,
    output_type: channelOutputType(channel.channel),
  });
  if (multiMode.value === 'live') {
    const cell = multiCells.value.find((item) => item.key === key);
    if (cell) await startMultiCell(cell);
  }
}
async function startMultiCell(cell: MultiViewCell) {
  const key = cell.key;
  const version = bumpMultiPlayVersion(key);
  const controller = new AbortController();
  multiPreviewAborts.get(key)?.abort();
  multiPreviewAborts.set(key, controller);
  try {
    const requestId = `ui-multi-${cell.mode}-${Date.now()}-${cell.channel_id}`;
    const stream = cell.mode === 'live'
      ? await startGbPreview(cell.device_id, cell.channel_id, {
          request_id: requestId,
          session_node_id: cell.session_node_id,
          output_type: cell.output_type,
          audio_codec: 'aac',
        }, {
          signal: controller.signal,
          onUpdate: (operation) => {
            if (multiPlayVersions[key] !== version || multiViewDisposed) return;
            const current = multiCells.value.find((item) => item.key === key);
            if (current) upsertMultiCell({ ...current, operation, status: 'reconnecting' });
          },
        })
      : await startGbPlayback(cell.device_id, cell.channel_id, {
          request_id: requestId,
          session_node_id: cell.session_node_id,
          playback_id: requestId,
          start_time_sec: cell.playback_position_sec ?? cell.playback_start_sec,
          end_time_sec: cell.playback_end_sec,
          output_type: 'fmp4',
          audio_codec: 'aac',
        }, {
      signal: controller.signal,
      onUpdate: (operation) => {
        if (multiPlayVersions[key] !== version || multiViewDisposed) return;
        const current = multiCells.value.find((item) => item.key === key);
        if (current) upsertMultiCell({ ...current, operation, status: 'reconnecting' });
      },
    });
    if (multiPlayVersions[key] !== version || !isMultiCellSelected(key) || multiViewDisposed) {
      await stopMultiStream(stream);
      return;
    }
    upsertMultiCell({
      ...cell,
      stream,
      sources: streamSources(stream, cell.mode),
      status: stream.state === 'running' ? 'playing' : 'online',
      playback_start_sec: cell.mode === 'playback' ? stream.playback_start_time_sec ?? cell.playback_start_sec : undefined,
      playback_end_sec: cell.mode === 'playback' ? stream.playback_end_time_sec ?? cell.playback_end_sec : undefined,
      playback_position_sec: cell.mode === 'playback' ? stream.playback_start_time_sec ?? cell.playback_position_sec : undefined,
      playback_generation: cell.mode === 'playback' ? stream.playback_generation ?? 0 : undefined,
      playback_rate: cell.mode === 'playback' ? cell.playback_rate ?? multiDesiredRate.value : undefined,
      playback_ack_rate: cell.mode === 'playback' ? 1 : undefined,
      playback_state: cell.mode === 'playback' ? 'playing' : undefined,
      operation: undefined,
      error: undefined,
    });
    if (cell.mode === 'playback' && (cell.playback_rate ?? multiDesiredRate.value) !== 1) {
      const current = multiCells.value.find((item) => item.key === key);
      if (current) await applyMultiPlaybackRate(current, cell.playback_rate ?? multiDesiredRate.value, false);
    }
  } catch (error) {
    if (multiPlayVersions[key] !== version || !isMultiCellSelected(key) || multiViewDisposed) return;
    upsertMultiCell({
      ...cell,
      sources: [],
      status: 'error',
      error: errorMessage(error, '播放失败'),
    });
  } finally {
    if (multiPreviewAborts.get(key) === controller) multiPreviewAborts.delete(key);
  }
}
function enqueueMultiPlayback(key: string) {
  const cell = multiCells.value.find((item) => item.key === key);
  if (!cell || cell.mode !== 'playback' || cell.stream || multiPlaybackQueue.value.includes(key)) return;
  multiPlaybackQueue.value.push(key);
  upsertMultiCell({ ...cell, status: 'queued', error: undefined });
  void drainMultiPlaybackQueue();
}
async function drainMultiPlaybackQueue() {
  if (multiPlaybackStarting.value || multiBulkBusy.value) return;
  multiPlaybackStarting.value = true;
  try {
    while (multiPlaybackQueue.value.length && !multiBulkBusy.value && !multiViewDisposed) {
      const key = multiPlaybackQueue.value.shift();
      if (!key) continue;
      const cell = multiCells.value.find((item) => item.key === key);
      const selected = selectedTreeChannelItems.value.find((item) => selectedChannelKey(item) === key);
      if (!cell || !selected?.playback_locked || cell.stream) continue;
      await startMultiCell(cell);
    }
  } finally {
    multiPlaybackStarting.value = false;
  }
}
function isMultiCellSelected(key: string) {
  return selectedTreeChannelKeys.value.includes(key) && multiCells.value.some((cell) => cell.key === key);
}
async function stopMultiStream(stream?: StreamSummary) {
  const streamId = stream?.stream_id;
  if (!streamId) return;
  const taskKey = `${streamId}:${stream.subscription_id || 'legacy'}`;
  const existing = multiStopTasks.get(taskKey);
  if (existing) return existing;
  let task: Promise<void>;
  task = releaseViewerStream(stream).catch(() => undefined).finally(() => {
    if (multiStopTasks.get(taskKey) === task) multiStopTasks.delete(taskKey);
  });
  multiStopTasks.set(taskKey, task);
  return task;
}

async function releaseViewerStream(stream: StreamSummary) {
  if (!stream.subscription_id) return;
  await releaseStream(
    stream.stream_id,
    stream.subscription_id,
    `ui-stream-release-${crypto.randomUUID()}`,
  );
}

async function closeTrackedOutputs(
  streamId: string,
  outputs: Array<StreamOutputSummary | undefined>,
) {
  const outputIds = [...new Set(outputs.map((output) => output?.output_id).filter((id): id is string => !!id))];
  await Promise.allSettled(outputIds.map((outputId) => closeStreamOutput(streamId, outputId)));
}

async function disposeMultiCellMedia(cell?: MultiViewCell) {
  if (!cell) return;
  if (cell.operation?.state === 'preparing') {
    await cancelMediaOperation(cell.operation.operation_id).catch(() => undefined);
  }
  if (cell.stream) {
    await closeTrackedOutputs(cell.stream.stream_id, [
      cell.output,
      cell.pending_switch?.previous_output,
      cell.pending_switch?.next_output,
    ]);
    await stopMultiStream(cell.stream);
  }
}
async function stopMultiCell(key: string, options: { removeSelection?: boolean } = {}) {
  const removeSelection = options.removeSelection !== false;
  bumpMultiPlayVersion(key);
  const cell = multiCells.value.find((item) => item.key === key);
  multiPreviewAborts.get(key)?.abort();
  multiPreviewAborts.delete(key);
  multiOutputAborts.get(key)?.abort();
  multiOutputAborts.delete(key);
  multiPlaybackQueue.value = multiPlaybackQueue.value.filter((item) => item !== key);
  multiCells.value = multiCells.value.filter((item) => item.key !== key);
  if (removeSelection) {
    selectedTreeChannelKeys.value = selectedTreeChannelKeys.value.filter((item) => item !== key);
    selectedTreeChannelItems.value = selectedTreeChannelItems.value.filter((item) => selectedChannelKey(item) !== key);
  }
  await disposeMultiCellMedia(cell);
}
async function stopAllMultiStreams(options: { quiet?: boolean } = {}) {
  if (multiStopping.value) return;
  const cells = [...multiCells.value];
  const streams = cells.map((cell) => cell.stream).filter((stream): stream is StreamSummary => !!stream?.stream_id);
  multiStopping.value = true;
  try {
    for (const cell of cells) bumpMultiPlayVersion(cell.key);
    for (const controller of multiPreviewAborts.values()) controller.abort();
    multiPreviewAborts.clear();
    for (const controller of multiOutputAborts.values()) controller.abort();
    multiOutputAborts.clear();
    multiCells.value = [];
    multiPlaybackQueue.value = [];
    selectedTreeChannelKeys.value = [];
    selectedTreeChannelItems.value = [];
    multiGridManual.value = false;
    multiGridSize.value = 1;
    multiPage.value = 1;
    await Promise.allSettled(cells.map((cell) => disposeMultiCellMedia(cell)));
    if (!options.quiet && streams.length) ElMessage.success('多画面已停止');
  } finally {
    multiStopping.value = false;
  }
}
async function stopCurrentStream(options: { closeDialog?: boolean; clearAction?: boolean; cancelPending?: boolean } = {}) {
  if (stopCurrentStreamTask) return stopCurrentStreamTask;
  const closeDialog = options.closeDialog !== false;
  const clearAction = options.clearAction !== false;
  const cancelPending = options.cancelPending !== false;
  stopCurrentStreamTask = (async () => {
    const stream = lastStream.value;
    const output = singleOutput.value;
    const pendingSwitch = singlePendingSwitch.value;
    if (cancelPending) {
      playRequestSeq += 1;
      singlePreviewAbort?.abort();
      singlePreviewAbort = undefined;
      singleOutputAbort?.abort();
      singleOutputAbort = undefined;
      playerRequesting.value = false;
      pendingPlayKey.value = '';
    }
    const operation = singleMediaOperation.value;
    if (closeDialog) playerDialog.value = false;
    lastStream.value = undefined;
    singleOutput.value = undefined;
    singlePendingSwitch.value = undefined;
    singleOutputSwitching.value = false;
    singleMediaOperation.value = undefined;
    singleWaitAcknowledged.value = false;
    if (clearAction) lastAction.value = '';
    if (operation?.state === 'preparing') {
      await cancelMediaOperation(operation.operation_id).catch(() => undefined);
    }
    if (stream?.stream_id) {
      await closeTrackedOutputs(stream.stream_id, [
        output,
        pendingSwitch?.previous_output,
        pendingSwitch?.next_output,
      ]);
      await releaseViewerStream(stream).catch(() => undefined);
    }
  })().finally(() => {
    stopCurrentStreamTask = undefined;
  });
  return stopCurrentStreamTask;
}
async function focusChannelInMultiView(channel: GbChannelInfo, mode: MultiMode = 'live') {
  const device = selectedDevice.value;
  if (!device) return;
  await stopCurrentStream();
  selectedDevice.value = undefined;
  selectedChannel.value = undefined;
  channels.value = [];
  images.value = [];
  showImages.value = false;
  multiMode.value = mode;
  multiDefaultRange.value = undefined;
  monitorMode.value = 'multi';
  await Promise.all([loadMultiViewCapability(), loadSessionNodes()]);
  const targetNodeId = device.session_node_id || selectedListNodeId.value;
  if (targetNodeId && selectedMultiNodeId.value !== targetNodeId) await selectMultiNode(targetNodeId);
  treeDeviceId.value = device.device_id;
  treeDeviceName.value = '';
  await queryTreeDevices();
  const targetDevice = treeDevices.value.find((item) => item.device_id === device.device_id) || treeDevices.value[0];
  if (targetDevice) {
    await loadTreeDeviceChannels(targetDevice);
    const targetChannel = (treeChannelsByDevice[targetDevice.device_id] || []).find((item) => item.channel_id === channel.channel_id);
    if (targetChannel) await toggleTreeChannel(targetChannel, true);
  }
  ElMessage.success('已定位到通道树');
}
async function focusSelectedMultiChannel(channel: SelectedChannelRef) {
  if (draggingTreeChannelIndex.value !== undefined) return;
  if (selectedMultiNodeId.value !== channel.session_node_id) {
    selectedMultiNodeId.value = channel.session_node_id;
    clearTreeDeviceBrowserState();
  }
  treeDeviceId.value = channel.device_id;
  treeDeviceName.value = '';
  treePage.value = 1;
  await queryTreeDevices();
  const device = treeDevices.value.find((item) => item.device_id === channel.device_id);
  if (!device) {
    ElMessage.warning('未在对应 Session 中找到该设备');
    return;
  }
  await loadTreeDeviceChannels(device);
  await nextTick();
  const deviceNode = multiDeviceTreeRef.value?.getNode(multiDeviceKey(channel.session_node_id, channel.device_id));
  if (deviceNode && !deviceNode.expanded) {
    await new Promise<void>((resolve) => deviceNode.expand(resolve));
  }
  await nextTick();
  multiDeviceTreeRef.value?.setCurrentKey(selectedChannelKey(channel));
  await nextTick();
  document.querySelector('.device-channel-tree .el-tree-node.is-current')?.scrollIntoView({ block: 'nearest' });
}
function multiCellAtVisibleIndex(index: number) {
  return multiCells.value[multiVisibleStart.value + index];
}
function handleMultiSnapshot(event: { payload: { fileName: string } }) {
  ElMessage.success('截图已保存：' + event.payload.fileName);
}
function handleMultiSnapshotError(event: { payload: { message: string } }) {
  ElMessage.error(event.payload.message);
}
function handleSingleSnapshot(event: { fileName: string }) {
  ElMessage.success('截图已保存：' + event.fileName);
}
function handleSingleSnapshotError(event: { message: string }) {
  ElMessage.error(event.message);
}
async function handleMultiClose(event: { index: number }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  await stopMultiCell(cell.key);
}
function handleMultiReorder(event: { sourceIndex: number; targetIndex: number }) {
  const sourceIndex = multiVisibleStart.value + event.sourceIndex;
  const targetIndex = multiVisibleStart.value + event.targetIndex;
  if (sourceIndex === targetIndex || !multiCells.value[sourceIndex] || !multiCells.value[targetIndex]) return;
  const cells = [...multiCells.value];
  const [cell] = cells.splice(sourceIndex, 1);
  if (!cell) return;
  cells.splice(targetIndex, 0, cell);
  multiCells.value = cells;
  syncSelectedTreeChannelsOrderFromCells();
}

function visibleMultiIndex(cell: MultiViewCell) {
  const index = multiCells.value.findIndex((item) => item.key === cell.key) - multiVisibleStart.value;
  return index >= 0 && index < multiGridSize.value ? index : undefined;
}
function requireMultiPlayback(cell?: MultiViewCell) {
  const playbackId = cell?.stream?.playback_id;
  const streamId = cell?.stream?.stream_id;
  return cell?.mode === 'playback' && playbackId && streamId ? { cell, playbackId, streamId } : undefined;
}
async function applyMultiPlaybackRate(cell: MultiViewCell, rate: number, notify = true) {
  const target = requireMultiPlayback(cell);
  if (!target) return false;
  if (cell.playback_state === 'paused') {
    upsertMultiCell({ ...cell, playback_rate: rate });
    const index = visibleMultiIndex(cell);
    if (index !== undefined) multiGridRef.value?.confirmPlaybackRate(index, rate);
    return true;
  }
  try {
    const result = await setGbPlaybackSpeed(target.playbackId, {
      request_id: `ui-multi-speed-${Date.now()}-${cell.channel_id}`,
      stream_id: target.streamId,
      speed_rate: rate,
      expected_generation: cell.playback_generation ?? 0,
    });
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current) upsertMultiCell({ ...current, playback_generation: result.generation, playback_rate: rate, playback_ack_rate: rate, error: undefined });
    const index = visibleMultiIndex(cell);
    if (index !== undefined) multiGridRef.value?.confirmPlaybackRate(index, rate);
    return true;
  } catch (error) {
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current) upsertMultiCell({ ...current, error: errorMessage(error, '倍速设置失败') });
    const index = visibleMultiIndex(cell);
    if (index !== undefined) multiGridRef.value?.confirmPlaybackRate(index, cell.playback_rate ?? 1);
    if (notify) ElMessage.error(errorMessage(error, `${cell.title} 倍速设置失败`));
    return false;
  }
}
async function applyMultiPlaybackState(cell: MultiViewCell, state: 'playing' | 'paused', notify = true) {
  const target = requireMultiPlayback(cell);
  if (!target) return false;
  if (state === 'playing' && (cell.playback_rate ?? 1) !== (cell.playback_ack_rate ?? 1)) {
    const applied = await applyMultiPlaybackRate({ ...cell, playback_state: 'playing' }, cell.playback_rate ?? 1, notify);
    if (applied) {
      const current = multiCells.value.find((item) => item.key === cell.key);
      if (current) upsertMultiCell({ ...current, playback_state: 'playing', status: 'playing' });
      const index = visibleMultiIndex(cell);
      if (index !== undefined) multiGridRef.value?.confirmPlaybackState(index, false);
    }
    return applied;
  }
  try {
    const result = await setGbPlaybackState(target.playbackId, {
      request_id: `ui-multi-state-${Date.now()}-${cell.channel_id}`,
      stream_id: target.streamId,
      paused: state === 'paused',
      expected_generation: cell.playback_generation ?? 0,
    });
    let current = multiCells.value.find((item) => item.key === cell.key);
    if (current) {
      current = { ...current, playback_generation: result.generation, playback_state: state, status: state === 'paused' ? 'paused' : 'playing', error: undefined };
      upsertMultiCell(current);
    }
    const index = visibleMultiIndex(cell);
    if (index !== undefined) multiGridRef.value?.confirmPlaybackState(index, state === 'paused');
    return true;
  } catch (error) {
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current) upsertMultiCell({ ...current, error: errorMessage(error, '播放状态设置失败') });
    const index = visibleMultiIndex(cell);
    if (index !== undefined) multiGridRef.value?.confirmPlaybackState(index, cell.playback_state === 'paused');
    if (notify) ElMessage.error(errorMessage(error, `${cell.title} 播放状态设置失败`));
    return false;
  }
}
async function handleMultiPlaybackRateChange(event: { index: number; payload: { rate: number } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (cell) await applyMultiPlaybackRate(cell, event.payload.rate);
}
async function handleMultiPlaybackStateChange(event: { index: number; payload: { paused: boolean } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (cell) await applyMultiPlaybackState(cell, event.payload.paused ? 'paused' : 'playing');
}
async function handleMultiPlaybackSeek(event: { index: number; payload: { timeMs: number } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  const target = requireMultiPlayback(cell);
  if (!target || !cell.playback_start_sec || !cell.playback_end_sec) return;
  const positionSec = Math.min(cell.playback_end_sec, Math.max(cell.playback_start_sec, cell.playback_start_sec + event.payload.timeMs / 1_000));
  try {
    const result = await seekGbPlayback(target.playbackId, {
      request_id: `ui-multi-seek-${Date.now()}-${cell.channel_id}`,
      stream_id: target.streamId,
      position_sec: positionSec,
      expected_generation: cell.playback_generation ?? 0,
    });
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current) upsertMultiCell({ ...current, playback_generation: result.generation, playback_position_sec: positionSec, error: undefined });
    multiGridRef.value?.confirmPlaybackProgress(event.index, (positionSec - cell.playback_start_sec) * 1_000);
  } catch (error) {
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current) upsertMultiCell({ ...current, error: errorMessage(error, '定位失败') });
    ElMessage.error(errorMessage(error, `${cell.title} 定位失败`));
    const currentMs = Math.max(0, ((cell.playback_position_sec ?? cell.playback_start_sec) - cell.playback_start_sec) * 1_000);
    multiGridRef.value?.confirmPlaybackProgress(event.index, currentMs);
  }
}
async function handleMultiPlaybackProgress(event: { index: number; payload: { mediaTimeMs: number } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell || cell.mode !== 'playback' || !cell.playback_start_sec || !cell.playback_end_sec) return;
  const positionSec = Math.min(cell.playback_end_sec, cell.playback_start_sec + event.payload.mediaTimeMs / 1_000);
  if (positionSec >= cell.playback_end_sec && cell.status !== 'stopped') {
    await disposeMultiCellMedia(cell);
    upsertMultiCell({ ...cell, stream: undefined, sources: [], operation: undefined, playback_position_sec: positionSec, playback_state: undefined, status: 'stopped' });
    return;
  }
  upsertMultiCell({ ...cell, playback_position_sec: positionSec });
}
async function toggleAllMultiPlayback() {
  if (multiBulkBusy.value || multiPlaybackStarting.value) {
    ElMessage.info('正在启动回放，请稍后操作');
    return;
  }
  const targetState = multiPauseActionLabel.value === '统一暂停' ? 'paused' : 'playing';
  const cells = multiControllableCells.value.filter((cell) => targetState === 'playing' ? cell.playback_state === 'paused' : cell.playback_state !== 'paused');
  multiBulkBusy.value = true;
  let failed = 0;
  try {
    for (const cell of cells) if (!await applyMultiPlaybackState(cell, targetState, false)) failed += 1;
  } finally {
    multiBulkBusy.value = false;
    void drainMultiPlaybackQueue();
  }
  if (failed) ElMessage.warning(`统一${targetState === 'paused' ? '暂停' : '继续'}完成，${failed} 路失败`);
}
async function setAllMultiPlaybackRate(value: number) {
  const rate = Number(value);
  if (!playbackRates.includes(rate) || multiBulkBusy.value || multiPlaybackStarting.value) return;
  multiDesiredRate.value = rate;
  multiBulkBusy.value = true;
  let failed = 0;
  try {
    for (const cell of multiControllableCells.value) if (!await applyMultiPlaybackRate(cell, rate, false)) failed += 1;
  } finally {
    multiBulkBusy.value = false;
    void drainMultiPlaybackQueue();
  }
  if (failed) ElMessage.warning(`统一倍速设置完成，${failed} 路失败`);
}

function asLiveOutputType(value: string): LiveOutputType | undefined {
  return value === 'flv' || value === 'hls' || value === 'fmp4' ? value : undefined;
}

function isAbortError(error: unknown) {
  return error instanceof DOMException && error.name === 'AbortError';
}

function mediaOperationStageText(stage?: string) {
  const labels: Record<string, string> = {
    accepted: '正在受理请求',
    waiting_device_response: '正在等待设备响应',
    waiting_media_input: '设备已响应，正在等待媒体数据',
    waiting_keyframe: '正在等待视频关键帧',
    building_output: '正在生成媒体输出',
    waiting_playlist: '正在生成 HLS 播放列表',
    attaching_player: '正在连接播放器',
    buffering_player: '播放器正在缓冲',
  };
  return labels[stage || ''] || '正在准备媒体';
}

function mediaOperationText(operation?: MediaOperationSummary<unknown>, acknowledged = false) {
  if (!operation) return '正在提交播放请求...';
  const elapsedSeconds = Math.max(0, Math.ceil(operation.elapsed_ms / 1_000));
  if (operation.checkpoint_ms > operation.elapsed_ms) {
    const remainingSeconds = Math.max(1, Math.ceil((operation.checkpoint_ms - operation.elapsed_ms) / 1_000));
    return elapsedSeconds < 3
      ? mediaOperationStageText(operation.stage)
      : `${mediaOperationStageText(operation.stage)}，${remainingSeconds} 秒后检查启动结果`;
  }
  const hardRemaining = Math.max(0, Math.ceil((operation.hard_timeout_ms - operation.elapsed_ms) / 1_000));
  return acknowledged
    ? `${mediaOperationStageText(operation.stage)}，继续等待中（最多 ${hardRemaining} 秒）`
    : `${mediaOperationStageText(operation.stage)}，尚未启动，是否继续等待？`;
}

async function acknowledgeSingleWait() {
  const operation = singleMediaOperation.value;
  if (!operation || operation.state !== 'preparing') return;
  singleWaitAcknowledged.value = true;
  singleMediaOperation.value = await continueMediaOperation(operation.operation_id).catch(() => operation);
}

async function cancelSingleStartup() {
  await stopCurrentStream({ closeDialog: false, clearAction: false });
}

async function handleMultiOutputTypeChange(event: { index: number; outputType: string }) {
  const cell = multiCellAtVisibleIndex(event.index);
  const outputType = asLiveOutputType(event.outputType);
  if (!cell || !outputType || cell.output_switching || cell.output_type === outputType) return;
  if (!cell.stream?.stream_id) {
    setChannelOutputType(cell.channel, outputType);
    upsertMultiCell({ ...cell, output_type: outputType });
    return;
  }
  const previousType = cell.output_type;
  const previousOutput = cell.output;
  const previousSources = cell.sources;
  const controller = new AbortController();
  multiOutputAborts.get(cell.key)?.abort();
  multiOutputAborts.set(cell.key, controller);
  upsertMultiCell({ ...cell, operation: undefined, output_switching: true, status: 'reconnecting', error: undefined });
  try {
    const nextOutput = await createStreamOutput(
      cell.stream.stream_id,
      outputType,
      `ui-multi-output-${Date.now()}-${cell.channel_id}-${outputType}`,
      {
        subscriptionId: cell.stream.subscription_id,
        signal: controller.signal,
        onUpdate: (operation) => {
          const current = multiCells.value.find((item) => item.key === cell.key);
          if (current && multiOutputAborts.get(cell.key) === controller) {
            upsertMultiCell({ ...current, operation });
          }
        },
      },
    );
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (!current?.stream || current.stream.stream_id !== cell.stream.stream_id) {
      await closeStreamOutput(cell.stream.stream_id, nextOutput.output_id).catch(() => undefined);
      return;
    }
    setChannelOutputType(cell.channel, outputType);
    upsertMultiCell({
      ...current,
      operation: undefined,
      output_type: outputType,
      output: nextOutput,
      sources: streamSources({ ...current.stream, endpoint: nextOutput.endpoint }),
      pending_switch: {
        previous_type: previousType,
        previous_output: previousOutput,
        previous_sources: previousSources,
        next_output: nextOutput,
      },
    });
  } catch (error) {
    setChannelOutputType(cell.channel, previousType);
    const current = multiCells.value.find((item) => item.key === cell.key) ?? cell;
    upsertMultiCell({
      ...current,
      operation: undefined,
      output_type: previousType,
      output_switching: false,
      status: cell.sources.length ? 'playing' : 'error',
      error: cell.sources.length ? undefined : errorMessage(error, '切换播放方式失败'),
    });
    if (!isAbortError(error)) ElMessage.error(errorMessage(error, '切换播放方式失败'));
  } finally {
    if (multiOutputAborts.get(cell.key) === controller) multiOutputAborts.delete(cell.key);
  }
}

async function handleMultiPlaying(event: { index: number }) {
  const cell = multiCellAtVisibleIndex(event.index);
  const pending = cell?.pending_switch;
  if (!cell || !pending) return;
  upsertMultiCell({ ...cell, pending_switch: undefined, output_switching: false, status: 'playing', error: undefined });
  if (pending.previous_output && pending.previous_output.output_id !== pending.next_output.output_id) {
    await closeStreamOutput(cell.stream!.stream_id, pending.previous_output.output_id).catch(() => undefined);
  }
}

async function handleMultiPlaybackError(event: { index: number; payload: { message: string } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  const pending = cell?.pending_switch;
  if (!cell) return;
  if (cell.mode === 'playback' && !pending) {
    await disposeMultiCellMedia(cell);
    upsertMultiCell({ ...cell, stream: undefined, sources: [], operation: undefined, status: 'error', error: event.payload.message });
    return;
  }
  if (!pending || !cell.stream) return;
  await closeStreamOutput(cell.stream.stream_id, pending.next_output.output_id).catch(() => undefined);
  setChannelOutputType(cell.channel, pending.previous_type);
  upsertMultiCell({
    ...cell,
    output_type: pending.previous_type,
    output: pending.previous_output,
    sources: pending.previous_sources,
    pending_switch: undefined,
    output_switching: false,
    status: 'reconnecting',
    error: undefined,
  });
  ElMessage.error(`切换播放方式失败：${event.payload.message}`);
}

async function handleMultiPlaybackSwitchCancel(event: { index: number }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  if (cell.mode === 'playback' && cell.operation?.state === 'preparing') {
    const selected = selectedTreeChannelItems.value.find((item) => selectedChannelKey(item) === cell.key);
    if (selected) await stopConfirmedMultiPlayback(selected);
    ElMessage.info('已取消该通道回放启动');
    return;
  }
  if (!cell.pending_switch && cell.operation?.state === 'preparing') {
    multiOutputAborts.get(cell.key)?.abort();
    await cancelMediaOperation(cell.operation.operation_id).catch(() => undefined);
    upsertMultiCell({
      ...cell,
      operation: undefined,
      output_switching: false,
      status: 'playing',
      error: undefined,
    });
    ElMessage.info('已保持当前播放方式');
    return;
  }
  const pending = cell?.pending_switch;
  if (!cell || !pending || !cell.stream) return;
  await closeStreamOutput(cell.stream.stream_id, pending.next_output.output_id).catch(() => undefined);
  setChannelOutputType(cell.channel, pending.previous_type);
  upsertMultiCell({
    ...cell,
    output_type: pending.previous_type,
    output: pending.previous_output,
    sources: pending.previous_sources,
    pending_switch: undefined,
    output_switching: false,
    status: 'playing',
    error: undefined,
  });
  ElMessage.info('已保持当前播放方式');
}

function ptzPayload(command: GmvPtzCommand): GbPtzPayload {
  const speed = Math.min(255, Math.max(1, Math.round(command.speed || 1)));
  const zoomSpeed = Math.min(15, speed);
  const payload: GbPtzPayload = { leftRight: 0, upDown: 0, inOut: 0, horizonSpeed: 0, verticalSpeed: 0, zoomSpeed: 0 };
  switch (command.action) {
    case 'left':
      payload.leftRight = 1;
      payload.horizonSpeed = speed;
      break;
    case 'right':
      payload.leftRight = 2;
      payload.horizonSpeed = speed;
      break;
    case 'up':
      payload.upDown = 1;
      payload.verticalSpeed = speed;
      break;
    case 'down':
      payload.upDown = 2;
      payload.verticalSpeed = speed;
      break;
    case 'leftUp':
      payload.leftRight = 1;
      payload.upDown = 1;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'rightUp':
      payload.leftRight = 2;
      payload.upDown = 1;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'leftDown':
      payload.leftRight = 1;
      payload.upDown = 2;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'rightDown':
      payload.leftRight = 2;
      payload.upDown = 2;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'zoomIn':
      payload.inOut = 2;
      payload.zoomSpeed = zoomSpeed;
      break;
    case 'zoomOut':
      payload.inOut = 1;
      payload.zoomSpeed = zoomSpeed;
      break;
  }
  return payload;
}

async function handleMultiPtz(event: { index: number; payload: GmvPtzCommand }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  try {
    await sendGbPtz(cell.device_id, cell.channel_id, ptzPayload(event.payload));
  } catch (error) {
    ElMessage.error(errorMessage(error, '云台控制失败'));
  }
}

async function loadSessionNodes() {
  const nodes = (await listNodes()).filter(isGbSessionNode);
  sessionNodes.value = nodes;
  listNodeLoading.value = true;
  try {
    const options = await Promise.all(nodes.map(async (node) => {
      if (!isNodeOnline(node)) return buildSessionNodeOption(node);
      try {
        return buildSessionNodeOption(node, await getGbSessionNodeConfig(node.node_id));
      } catch {
        return buildSessionNodeOption(node, undefined, '配置查询失败');
      }
    }));
    sessionNodeOptions.value = options.sort((left, right) => Number(left.disabled) - Number(right.disabled) || left.node.node_id.localeCompare(right.node.node_id));
    if (!selectedListNodeId.value || !sessionNodeOptions.value.some((item) => item.node.node_id === selectedListNodeId.value && !item.disabled)) {
      selectedListNodeId.value = sessionNodeOptions.value.find((item) => !item.disabled)?.node.node_id || '';
    }
  } finally {
    listNodeLoading.value = false;
  }
}
async function loadDevices() {
  loading.value = true;
  try {
    await loadSessionNodes();
    const option = selectedListNodeOption.value;
    if (!option || option.disabled || !option.config?.domain_id) {
      devices.value = [];
      total.value = 0;
      return;
    }
    const result = await listGbDevicePage(
      page.value,
      pageSize.value,
      option.node.node_id,
      option.config.domain_id,
      '',
      deviceName.value,
      true,
    );
    devices.value = result.items;
    total.value = result.total;
    page.value = result.page;
    pageSize.value = result.page_size;
  } catch (error) {
    ElMessage.error(errorMessage(error, '设备列表加载失败'));
  } finally {
    loading.value = false;
  }
}
async function queryDevices() { page.value = 1; await loadDevices(); }
async function resetDevices() { deviceName.value = ''; page.value = 1; await loadDevices(); }
async function handlePageSizeChange() { page.value = 1; await loadDevices(); }
async function handleListNodeChange() { page.value = 1; await loadDevices(); }
function openDeviceDetail(device: GbDeviceInfo) {
  detailDevice.value = device;
  deviceDetailDrawer.value = true;
}
async function openChannelsFromDetail() {
  if (!detailDevice.value) return;
  const device = detailDevice.value;
  deviceDetailDrawer.value = false;
  await openChannels(device);
}
async function openChannels(device: GbDeviceInfo) {
  await stopCurrentStream();
  selectedDevice.value = device;
  selectedChannel.value = undefined;
  showImages.value = false;
  await reloadChannels();
}
async function reloadChannels() {
  if (!selectedDevice.value) return;
  channelLoading.value = true;
  resourceLoading.value = true;
  try {
    const [channelRows, resourceRows] = await Promise.all([
      listGbChannels(selectedDevice.value.device_id, selectedDevice.value.session_node_id),
      listGbResources(selectedDevice.value.device_id, selectedDevice.value.session_node_id),
    ]);
    channels.value = channelRows;
    resources.value = resourceRows;
    resourcesLoaded.value = true;
  } catch (error) {
    ElMessage.error(errorMessage(error, '通道列表加载失败'));
  } finally {
    channelLoading.value = false;
    resourceLoading.value = false;
  }
}
async function backToDevices() {
  await stopBroadcast();
  await stopCurrentStream();
  selectedDevice.value = undefined;
  selectedChannel.value = undefined;
  channels.value = [];
  resources.value = [];
  resourcesLoaded.value = false;
  images.value = [];
  showImages.value = false;
}
async function handleSingleOutputTypeChange(value: string) {
  const outputType = asLiveOutputType(value);
  const channel = selectedChannel.value;
  const stream = lastStream.value;
  if (!outputType || !channel || !stream?.stream_id || singleOutputSwitching.value) return;
  const previousType = channelOutputType(channel);
  if (previousType === outputType) return;
  const controller = new AbortController();
  singleOutputAbort?.abort();
  singleOutputAbort = controller;
  singleOutputSwitching.value = true;
  singleMediaOperation.value = undefined;
  singleWaitAcknowledged.value = false;
  try {
    const nextOutput = await createStreamOutput(
      stream.stream_id,
      outputType,
      `ui-single-output-${Date.now()}-${channel.channel_id}-${outputType}`,
      {
        subscriptionId: stream.subscription_id,
        signal: controller.signal,
        onUpdate: (operation) => {
          if (singleOutputAbort === controller) singleMediaOperation.value = operation;
        },
      },
    );
    if (lastStream.value?.stream_id !== stream.stream_id) {
      await closeStreamOutput(stream.stream_id, nextOutput.output_id).catch(() => undefined);
      return;
    }
    singlePendingSwitch.value = {
      previous_type: previousType,
      previous_output: singleOutput.value,
      previous_endpoint: stream.endpoint,
      next_output: nextOutput,
    };
    singleOutput.value = nextOutput;
    singleMediaOperation.value = undefined;
    setChannelOutputType(channel, outputType);
    lastStream.value = { ...stream, endpoint: nextOutput.endpoint };
  } catch (error) {
    singleMediaOperation.value = undefined;
    singleOutputSwitching.value = false;
    if (!isAbortError(error)) ElMessage.error(errorMessage(error, '切换播放方式失败'));
  } finally {
    if (singleOutputAbort === controller) singleOutputAbort = undefined;
  }
}

async function handleSinglePlaying() {
  const pending = singlePendingSwitch.value;
  const stream = lastStream.value;
  if (!pending || !stream) return;
  singlePendingSwitch.value = undefined;
  singleOutputSwitching.value = false;
  if (pending.previous_output && pending.previous_output.output_id !== pending.next_output.output_id) {
    await closeStreamOutput(stream.stream_id, pending.previous_output.output_id).catch(() => undefined);
  }
}

async function handleSinglePlaybackError(event: { message: string }) {
  const pending = singlePendingSwitch.value;
  const stream = lastStream.value;
  const channel = selectedChannel.value;
  if (!pending || !stream || !channel) return;
  await closeStreamOutput(stream.stream_id, pending.next_output.output_id).catch(() => undefined);
  setChannelOutputType(channel, pending.previous_type);
  singleOutput.value = pending.previous_output;
  singlePendingSwitch.value = undefined;
  singleOutputSwitching.value = false;
  lastStream.value = { ...stream, endpoint: pending.previous_endpoint };
  ElMessage.error(`切换播放方式失败：${event.message}`);
}

async function handleSinglePlaybackSwitchCancel() {
  const pending = singlePendingSwitch.value;
  const stream = lastStream.value;
  const channel = selectedChannel.value;
  const operation = singleMediaOperation.value;
  if (!pending && operation?.state === 'preparing') {
    singleOutputAbort?.abort();
    await cancelMediaOperation(operation.operation_id).catch(() => undefined);
    singleMediaOperation.value = undefined;
    singleOutputSwitching.value = false;
    ElMessage.info('已保持当前播放方式');
    return;
  }
  if (!pending || !stream || !channel) return;
  await closeStreamOutput(stream.stream_id, pending.next_output.output_id).catch(() => undefined);
  setChannelOutputType(channel, pending.previous_type);
  singleOutput.value = pending.previous_output;
  singlePendingSwitch.value = undefined;
  singleOutputSwitching.value = false;
  lastStream.value = { ...stream, endpoint: pending.previous_endpoint };
  ElMessage.info('已保持当前播放方式');
}

function requestPlayback(channel: GbChannelInfo) {
  pendingPlaybackChannel.value = channel;
  playbackRange.value = undefined;
  playbackRangeDialog.value = true;
}

async function confirmPlaybackRange() {
  const channel = pendingPlaybackChannel.value;
  const range = playbackRange.value;
  if (!channel || !range || range[0].getTime() >= range[1].getTime()) {
    ElMessage.warning('请选择有效的回放开始和结束时间');
    return;
  }
  playbackRangeDialog.value = false;
  await startPlay('playback', channel, range);
}

async function startPlay(kind: 'preview' | 'playback', channel: GbChannelInfo, range?: [Date, Date]) {
  if (playerRequesting.value) return;
  const action = kind === 'preview' ? '实时直播' : '历史回放';
  const requestSeq = playRequestSeq + 1;
  playRequestSeq = requestSeq;
  selectedChannel.value = channel;
  lastAction.value = action;
  showImages.value = false;
  playerDialog.value = true;
  playerRequesting.value = true;
  singleMediaOperation.value = undefined;
  singleWaitAcknowledged.value = false;
  pendingPlayKey.value = playRequestKey(kind, channel);
  try {
    await stopCurrentStream({ closeDialog: false, clearAction: false, cancelPending: false });
    const controller = new AbortController();
    singlePreviewAbort?.abort();
    singlePreviewAbort = controller;
    const playbackRequestId = 'ui-monitor-playback-' + Date.now();
    const stream = kind === 'preview'
      ? await startGbPreview(
          channel.device_id,
          channel.channel_id,
          { request_id: 'ui-monitor-preview-' + Date.now(), session_node_id: selectedDevice.value?.session_node_id, output_type: channelOutputType(channel), audio_codec: 'aac' },
          {
            signal: controller.signal,
            onUpdate: (operation) => {
              if (requestSeq === playRequestSeq) singleMediaOperation.value = operation;
            },
          },
        )
      : await startGbPlayback(
          channel.device_id,
          channel.channel_id,
          { request_id: playbackRequestId, session_node_id: selectedDevice.value?.session_node_id, playback_id: playbackRequestId, start_time_sec: Math.floor(range![0].getTime() / 1000), end_time_sec: Math.floor(range![1].getTime() / 1000), output_type: 'fmp4', audio_codec: 'aac' },
          {
            signal: controller.signal,
            onUpdate: (operation) => {
              if (requestSeq === playRequestSeq) singleMediaOperation.value = operation;
            },
          },
        );
    if (requestSeq !== playRequestSeq || !playerDialog.value) {
      if (stream.stream_id) await releaseViewerStream(stream).catch(() => undefined);
      return;
    }
    lastStream.value = stream;
    if (kind === 'playback') {
      playbackGeneration.value = stream.playback_generation ?? 0;
      playbackAnchorPositionSec.value = stream.playback_start_time_sec ?? Math.floor(range![0].getTime() / 1000);
      playbackAnchorMediaTimeMs.value = undefined;
      playbackLastMediaTimeMs.value = undefined;
      playbackDisplayedPositionSec.value = playbackAnchorPositionSec.value;
      playbackRangeEnded.value = false;
    }
    singleOutput.value = undefined;
    singlePendingSwitch.value = undefined;
    singleOutputSwitching.value = false;
    singleMediaOperation.value = undefined;
    ElMessage.success(action + '已提交');
  } catch (error) {
    if (requestSeq === playRequestSeq) ElMessage.error(errorMessage(error, '播放请求失败'));
  } finally {
    if (requestSeq === playRequestSeq) {
      playerRequesting.value = false;
      pendingPlayKey.value = '';
      singlePreviewAbort = undefined;
    }
  }
}
async function requestDeviceSnapshot(channel: GbChannelInfo) {
  if (deviceSnapshotLoading[channel.channel_id]) return;
  deviceSnapshotLoading[channel.channel_id] = true;
  try {
    await takeGbSnapshot(channel.device_id, channel.channel_id);
    selectedChannel.value = channel;
    await loadImages(channel);
    ElMessage.success('抓拍已提交');
  } catch (error) {
    ElMessage.error(errorMessage(error, '抓拍失败'));
  } finally {
    deviceSnapshotLoading[channel.channel_id] = false;
  }
}
async function loadImages(channel: GbChannelInfo) {
  imageLoading.value = true;
  try {
    images.value = await listGbChannelImages(channel.device_id, channel.channel_id);
  } catch (error) {
    images.value = [];
    ElMessage.error(errorMessage(error, '抓拍图集加载失败'));
  } finally {
    imageLoading.value = false;
  }
}
async function openImages(channel: GbChannelInfo) {
  selectedChannel.value = channel;
  showImages.value = true;
  await loadImages(channel);
}
function previewCover(channel: GbChannelInfo) {
  coverUrl.value = channel.pic_url || '';
  coverDialog.value = true;
}
function openConfig(channel: GbChannelInfo) {
  selectedChannel.value = channel;
  Object.assign(configForm, {
    device_id: channel.device_id,
    channel_id: channel.channel_id,
    name: channel.name,
    alias_name: channel.alias_name || '',
    ptz_enable: confValue(channel.ptz_enable),
    talk_enable: confValue(channel.talk_enable),
    audio_enable: confValue(channel.audio_enable),
    snapshot: confValue(channel.snapshot),
    record_enable: confValue(channel.record_enable),
    playback_enable: confValue(channel.playback_enable),
    alarm_enable: confValue(channel.alarm_enable),
    biz_enable: confValue(channel.biz_enable, 1),
    sort_no: Number(channel.sort_no || 0),
  });
  configDrawer.value = true;
}
async function openResourceDrawer() {
  resourceDrawer.value = true;
  if (!selectedDevice.value) return;
  resourceLoading.value = true;
  try {
    resources.value = await listGbResources(selectedDevice.value.device_id, selectedDevice.value.session_node_id);
    resourcesLoaded.value = true;
  } catch (error) {
    ElMessage.error(errorMessage(error, '资源能力加载失败'));
  } finally {
    resourceLoading.value = false;
  }
}
function editResource(resource: GbResourceInfo) {
  if (!canManageResources.value) return;
  resourceEditing.value = resource;
  Object.assign(resourceForm, {
    resource_kind: (resource.confirmation?.status === 1 ? resource.confirmation.resource_kind : resource.effective_kind) as typeof resourceForm.resource_kind,
    owner_scope: (resource.confirmation?.status === 1 ? resource.confirmation.owner_scope : resource.effective_owner_scope) as typeof resourceForm.owner_scope,
    owner_id: resource.confirmation?.status === 1 ? resource.confirmation.owner_id : resource.effective_owner_id,
    remark: resource.confirmation?.remark || '',
  });
  syncResourceOwner();
  resourceEditDialog.value = true;
}
function syncResourceOwner() {
  if (!selectedDevice.value) return;
  if (resourceForm.owner_scope === 'device') resourceForm.owner_id = selectedDevice.value.device_id;
  else if (!ownerResourceOptions.value.some((channel) => channel.channel_id === resourceForm.owner_id)) resourceForm.owner_id = ownerResourceOptions.value[0]?.channel_id || '';
}
async function saveResource() {
  if (!selectedDevice.value || !resourceEditing.value || !resourceForm.owner_id) return;
  resourceSaving.value = true;
  try {
    await saveGbResourceConfirmation(selectedDevice.value.device_id, resourceEditing.value.resource_id, {
      request_id: `ui-resource-confirm-${Date.now()}`,
      resource_kind: resourceForm.resource_kind,
      owner_scope: resourceForm.owner_scope,
      owner_id: resourceForm.owner_id,
      remark: resourceForm.remark,
    });
    resourceEditDialog.value = false;
    await openResourceDrawer();
    ElMessage.success('人工覆盖已保存');
  } catch (error) {
    ElMessage.error(errorMessage(error, '人工覆盖保存失败'));
  } finally {
    resourceSaving.value = false;
  }
}
async function resetResource(resource: GbResourceInfo) {
  if (!selectedDevice.value || !canManageResources.value) return;
  resourceSaving.value = true;
  try {
    await resetGbResourceConfirmation(selectedDevice.value.device_id, resource.resource_id, `ui-resource-reset-${Date.now()}`);
    await openResourceDrawer();
    ElMessage.success('已恢复自动识别');
  } catch (error) {
    ElMessage.error(errorMessage(error, '恢复自动识别失败'));
  } finally {
    resourceSaving.value = false;
  }
}
async function startBroadcast(scopeId: string) {
  if (!selectedDevice.value || broadcastStarting.value || broadcastSession.value) return;
  broadcastStarting.value = true;
  broadcastScopeId.value = scopeId;
  try {
    broadcastSession.value = await startGbMicrophoneBroadcast(selectedDevice.value.device_id, scopeId);
    ElMessage.success('语音广播已开始');
  } catch (error) {
    ElMessage.error(errorMessage(error, '语音广播启动失败'));
  } finally {
    broadcastStarting.value = false;
  }
}
async function stopBroadcast() {
  const session = broadcastSession.value;
  if (!session) return;
  broadcastSession.value = undefined;
  broadcastStarting.value = true;
  try {
    await session.stop();
    ElMessage.success('语音广播已停止');
  } finally {
    broadcastScopeId.value = '';
    broadcastStarting.value = false;
  }
}
async function saveConfig() {
  if (!selectedChannel.value) return;
  configSaving.value = true;
  try {
    const payload = { ...configForm };
    delete payload.device_id;
    await updateGbChannel(selectedChannel.value.device_id, selectedChannel.value.channel_id, payload);
    configDrawer.value = false;
    await reloadChannels();
    ElMessage.success('业务配置已保存');
  } catch (error) {
    ElMessage.error(errorMessage(error, '业务配置保存失败'));
  } finally {
    configSaving.value = false;
  }
}
async function handlePlaybackRateChange({ rate }: { rate: number }) {
  const stream = lastStream.value;
  if (!stream?.stream_id || !stream.playback_id) return;
  try {
    const response = await setGbPlaybackSpeed(stream.playback_id, {
      request_id: 'ui-playback-speed-' + Date.now(),
      stream_id: stream.stream_id,
      speed_rate: rate,
      expected_generation: playbackGeneration.value,
    });
    playbackGeneration.value = response.generation;
    singlePlayerRef.value?.confirmPlaybackRate(rate);
    singlePlayerRef.value?.confirmPlaybackState(false);
    ElMessage.success(`回放倍速已切换为 ${rate}x`);
  } catch (error) {
    ElMessage.error(errorMessage(error, '回放倍速设置失败'));
  }
}

async function handlePlaybackSeek({ timeMs }: { timeMs: number }) {
  queuedSeekMs = timeMs;
  if (seekInFlight) return;
  seekInFlight = true;
  try {
    while (queuedSeekMs !== undefined) {
      const targetMs = queuedSeekMs;
      queuedSeekMs = undefined;
      const stream = lastStream.value;
      if (!stream?.stream_id || !stream.playback_id || !stream.playback_start_time_sec) return;
      const targetSec = stream.playback_start_time_sec + Math.floor(targetMs / 1000);
      const response = await seekGbPlayback(stream.playback_id, {
        request_id: 'ui-playback-seek-' + Date.now(),
        stream_id: stream.stream_id,
        position_sec: targetSec,
        expected_generation: playbackGeneration.value,
      });
      playbackGeneration.value = response.generation;
      playbackAnchorPositionSec.value = targetSec;
      playbackAnchorMediaTimeMs.value = undefined;
      playbackLastMediaTimeMs.value = undefined;
      playbackDisplayedPositionSec.value = targetSec;
      singlePlayerRef.value?.confirmPlaybackProgress(targetMs);
      singlePlayerRef.value?.confirmPlaybackState(false);
    }
  } catch (error) {
    queuedSeekMs = undefined;
    ElMessage.error(errorMessage(error, '回放定位失败'));
  } finally {
    seekInFlight = false;
  }
}

async function handlePlaybackStateChange({ paused }: { paused: boolean }) {
  const stream = lastStream.value;
  if (!stream?.stream_id || !stream.playback_id) return;
  try {
    const response = await setGbPlaybackState(stream.playback_id, {
      request_id: 'ui-playback-state-' + Date.now(),
      stream_id: stream.stream_id,
      paused,
      expected_generation: playbackGeneration.value,
    });
    playbackGeneration.value = response.generation;
    singlePlayerRef.value?.confirmPlaybackState(paused);
  } catch (error) {
    ElMessage.error(errorMessage(error, paused ? '暂停回放失败' : '继续回放失败'));
  }
}

function handlePlaybackProgress({ mediaTimeMs }: { mediaTimeMs: number }) {
  const stream = lastStream.value;
  if (!stream?.playback_start_time_sec || !stream.playback_end_time_sec) return;
  if (playbackLastMediaTimeMs.value !== undefined && mediaTimeMs + 1000 < playbackLastMediaTimeMs.value) {
    playbackAnchorPositionSec.value = playbackDisplayedPositionSec.value;
    playbackAnchorMediaTimeMs.value = mediaTimeMs;
  }
  playbackLastMediaTimeMs.value = mediaTimeMs;
  if (playbackAnchorMediaTimeMs.value === undefined) playbackAnchorMediaTimeMs.value = mediaTimeMs;
  const elapsedMs = Math.max(0, mediaTimeMs - playbackAnchorMediaTimeMs.value);
  const positionSec = Math.min(stream.playback_end_time_sec, playbackAnchorPositionSec.value + elapsedMs / 1000);
  playbackDisplayedPositionSec.value = positionSec;
  singlePlayerRef.value?.confirmPlaybackProgress((positionSec - stream.playback_start_time_sec) * 1000);
  if (positionSec >= stream.playback_end_time_sec && !playbackRangeEnded.value) {
    playbackRangeEnded.value = true;
    void finishPlaybackRange(stream);
  }
}

async function finishPlaybackRange(stream: StreamSummary) {
  singlePlayerRef.value?.confirmPlaybackState(true);
  await releaseViewerStream(stream).catch(() => undefined);
  if (lastStream.value?.stream_id === stream.stream_id) {
    lastStream.value = { ...stream, endpoint: '', state: 'stopped' };
    ElMessage.success('历史回放已结束');
  }
}

async function handlePlayerPtz(command: GmvPtzCommand) {
  if (!selectedChannel.value) return;
  try {
    await sendGbPtz(selectedChannel.value.device_id, selectedChannel.value.channel_id, ptzPayload(command));
  } catch (error) {
    ElMessage.error(errorMessage(error, '云台控制失败'));
  }
}

onMounted(loadDevices);
onBeforeRouteLeave(async () => {
  multiViewDisposed = true;
  await stopBroadcast();
  await stopAllMultiStreams({ quiet: true });
});
onBeforeUnmount(() => {
  multiViewDisposed = true;
  void stopBroadcast();
  void stopAllMultiStreams({ quiet: true });
  void stopCurrentStream();
});
</script>

<style scoped>
.node-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
}

.node-status {
  color: var(--cyan);
  font-size: 12px;
}

.node-option.offline {
  color: var(--muted);
}

.node-option.offline .node-status {
  color: var(--muted);
}

.pagination-bar {
  display: flex;
  justify-content: flex-end;
  padding-top: 14px;
}

.monitor-head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 14px;
  align-items: center;
}

.device-summary,
.monitor-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.device-summary strong {
  font-size: 17px;
}

.device-summary span {
  color: var(--muted);
  font-size: 13px;
}

.multi-node-select {
  width: 420px;
  max-width: 100%;
}

.multi-default-range {
  width: 340px;
  max-width: 100%;
}

.selected-channel-capacity {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--cyan);
  font-size: 13px;
  font-weight: 700;
}

.multi-limit-help {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: 0;
  color: var(--cyan);
  background: transparent;
  cursor: help;
}

.multi-limit-help:hover {
  color: var(--el-color-primary-light-3);
}

.multi-limit-help:focus-visible {
  border-radius: 50%;
  color: var(--el-color-primary-light-3);
  outline: 1px solid var(--component-border-strong);
}

.multi-player-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.multi-rate-select {
  width: 88px;
}

.multi-player-summary {
  display: flex;
  align-items: baseline;
  gap: 10px;
  min-width: 0;
  white-space: nowrap;
}

.multi-player-summary > strong {
  font-size: 16px;
}

.multi-player-summary > span {
  overflow: hidden;
  color: var(--muted);
  font-size: 12px;
  text-overflow: ellipsis;
}

.channel-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 14px;
  min-height: 160px;
}

.channel-card {
  min-width: 0;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
}

.channel-card-head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 10px;
  align-items: start;
  padding: 12px;
}

.channel-card-head h2 {
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-size: 16px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.channel-card-head p {
  margin: 5px 0 0;
  overflow: hidden;
  color: var(--muted);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.channel-cover {
  display: grid;
  place-items: center;
  width: 100%;
  aspect-ratio: 4 / 3;
  border: 0;
  border-top: 1px solid rgba(100, 203, 255, .12);
  border-bottom: 1px solid rgba(100, 203, 255, .12);
  background: rgba(2, 5, 10, .88);
  color: var(--muted);
  cursor: pointer;
}

.channel-cover:disabled {
  cursor: default;
}

.channel-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.channel-tags {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  padding: 10px 12px 0;
}

.channel-tags span {
  padding: 4px 8px;
  border: 1px solid var(--component-border);
  border-radius: 999px;
  color: var(--cyan);
  background: var(--component-bg-soft);
  font-size: 12px;
}

.channel-actions {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 4px;
  padding: 12px;
}

.channel-actions .el-button {
  width: 100%;
  margin-left: 0;
}

.channel-play-entry {
  display: flex;
  width: 100%;
  min-width: 0;
}

.channel-play-entry .el-button {
  width: auto;
  height: 32px;
  padding: 7px 9px;
}

.channel-play-entry .channel-play-main {
  flex: 1 1 auto;
  min-width: 0;
  font-size: 13px;
  font-weight: 700;
}

.channel-play-entry .channel-multi-tag {
  flex: 0 0 34px;
  width: 34px;
  padding: 0;
  font-size: 12px;
  font-weight: 800;
}

.channel-play-entry .channel-multi-tag:not(.is-disabled) {
  color: #67e8f9;
  background: rgba(8, 145, 178, .28);
}

.channel-play-entry .el-button:not(.is-disabled):hover {
  filter: brightness(1.18);
}

.channel-second-row {
  grid-column-start: 1;
}

.channel-output-select {
  grid-column: span 2;
  width: 100%;
  min-width: 0;
}

:deep(.monitor-player-dialog .el-dialog__body) {
  padding: 18px 20px 20px;
}

.monitor-player {
  display: grid;
  grid-template-rows: minmax(0, 1fr);
  min-height: 560px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
  padding: 10px;
}

.monitor-player-stage {
  position: relative;
  min-height: 500px;
  overflow: hidden;
  border-radius: 8px;
}

.player-loading-badge {
  position: absolute;
  left: 16px;
  bottom: 16px;
  z-index: 5;
  padding: 7px 10px;
  border: 1px solid rgba(100, 203, 255, .36);
  border-radius: 6px;
  background: rgba(2, 8, 16, .86);
  color: var(--text);
  font-size: 12px;
  letter-spacing: 0;
}

.player-loading-badge span {
  display: block;
}

.player-loading-actions {
  display: flex;
  gap: 6px;
  margin-top: 4px;
}

.monitor-player :deep(.gmv-player) {
  position: relative;
  width: 100%;
  height: 100%;
  min-height: 500px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
  color: var(--text);
}

.monitor-player :deep(.gmv-video) {
  width: 100%;
  height: 100%;
  display: block;
  object-fit: contain;
  background: #02050a;
}

.monitor-player :deep(.media-info-panel),
.multi-player :deep(.media-info-panel) {
  position: absolute;
  right: 10px;
  bottom: 52px;
  z-index: 5;
  display: grid;
  gap: 7px;
  width: min(360px, calc(100% - 20px));
  max-height: calc(100% - 64px);
  padding: 10px 12px;
  overflow: auto;
  border: 1px solid rgba(100, 203, 255, .22);
  border-radius: 8px;
  background: rgba(3, 10, 24, .92);
  box-shadow: 0 14px 36px rgba(0, 0, 0, .42);
}

.monitor-player :deep(.gmv-player.has-playback-timeline .media-info-panel) {
  bottom: 112px;
  max-height: calc(100% - 124px);
}

.monitor-player :deep(.media-info-row),
.multi-player :deep(.media-info-row) {
  display: grid;
  grid-template-columns: 88px minmax(0, 1fr);
  gap: 10px;
}

.monitor-player :deep(.media-info-panel span),
.multi-player :deep(.media-info-panel span) {
  color: var(--muted);
  font-size: 12px;
}

.monitor-player :deep(.media-info-panel b),
.multi-player :deep(.media-info-panel b) {
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 12px;
  font-weight: 600;
}

.monitor-player :deep(.media-info-diagnostics),
.multi-player :deep(.media-info-diagnostics) {
  padding-top: 7px;
  border-top: 1px solid rgba(100, 203, 255, .22);
}

.monitor-player :deep(.media-info-diagnostics summary),
.multi-player :deep(.media-info-diagnostics summary) {
  cursor: pointer;
  color: var(--cyan);
  font-size: 12px;
}

.monitor-player :deep(.media-info-diagnostic-list),
.multi-player :deep(.media-info-diagnostic-list) {
  display: grid;
  gap: 7px;
  margin-top: 7px;
}

.monitor-player :deep(.reconnect-banner) {
  position: absolute;
  top: 52px;
  left: 12px;
  right: 12px;
  z-index: 3;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 1px solid rgba(244, 180, 0, .45);
  border-radius: 6px;
  background: rgba(52, 33, 9, .82);
}

.monitor-player :deep(.ptz-panel) {
  position: absolute;
  top: 74px;
  right: 12px;
  z-index: 4;
  width: 156px;
  padding: 10px;
  border: 1px solid rgba(100, 203, 255, .22);
  border-radius: 8px;
  background: rgba(3, 10, 24, .86);
  box-shadow: 0 14px 36px rgba(0, 0, 0, .32);
}

.monitor-player :deep(.ptz-grid) {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 5px;
}

.monitor-player :deep(.ptz-grid button) {
  aspect-ratio: 1;
  border-radius: 5px;
}

.monitor-player :deep(.ptz-panel label) {
  display: grid;
  gap: 5px;
  margin: 8px 0;
  color: var(--muted);
  font-size: 12px;
}

.monitor-player :deep(.lens-row) {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 5px;
  margin-top: 5px;
}

.tree-device-list,
.tree-workbench {
  display: grid;
  gap: 12px;
}

.tree-device-list {
  height: 352px;
  align-content: start;
  overflow: auto;
}

.tree-channel-label small {
  color: var(--muted);
  font-size: 12px;
}

.tree-channel-label b {
  display: block;
  overflow-wrap: anywhere;
}

.device-channel-tree {
  min-width: 0;
  background: transparent;
  color: var(--text);
}

.device-channel-tree :deep(.el-tree-node__content) {
  min-width: 0;
  height: auto;
  min-height: 42px;
  border-radius: 8px;
  color: var(--text);
}

.device-channel-tree :deep(.el-tree-node__content:hover),
.device-channel-tree :deep(.el-tree-node:focus > .el-tree-node__content) {
  background: rgba(34, 211, 238, .07);
}

.device-channel-tree :deep(.el-tree-node__expand-icon) {
  color: var(--muted);
}

.device-channel-tree :deep(.el-tree-node__expand-icon.expanded) {
  color: var(--cyan);
}

.tree-device-node {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  width: 100%;
  min-width: 0;
  padding: 8px 10px 8px 0;
}

.tree-device-title,
.tree-channel-label {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.tree-device-title b,
.tree-channel-label b,
.tree-channel-label small {
  overflow-wrap: anywhere;
}

.tree-channel-node {
  width: 100%;
  min-width: 0;
  height: auto;
  padding: 7px 10px 7px 0;
  white-space: normal;
}

.tree-channel-node :deep(.el-checkbox__label) {
  min-width: 0;
  white-space: normal;
}

.tree-pagination {
  justify-content: center;
  padding-top: 2px;
}

.selected-channel-panel {
  display: grid;
  grid-template-rows: minmax(0, 1fr);
  min-height: 428px;
}

.selected-channel-list {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
  align-content: start;
  height: 352px;
  overflow: auto;
}

.selected-channel-list.playback {
  grid-template-columns: minmax(0, 1fr);
}

.selected-channel-item {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  padding: 12px;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
  cursor: grab;
  user-select: none;
}

.selected-channel-list.playback .selected-channel-item {
  grid-template-areas:
    "main remove"
    "playback playback";
  grid-template-columns: minmax(0, 1fr) auto;
  row-gap: 8px;
}

.selected-channel-main,
.selected-channel-playback {
  display: grid;
  gap: 8px;
  min-width: 0;
}

.selected-channel-main b {
  font-size: 13px;
}

.selected-channel-playback :deep(.el-date-editor) {
  width: 100%;
}

.selected-channel-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.selected-channel-item.dragging {
  opacity: .55;
}

.selected-channel-item b,
.selected-channel-item span {
  display: block;
  overflow-wrap: anywhere;
}

.selected-channel-item span {
  color: var(--muted);
  font-size: 12px;
}

.selected-channel-list.playback .selected-channel-main {
  grid-area: main;
  display: grid;
  grid-template-columns: 24px minmax(0, 1fr) auto;
  align-items: center;
}

.selected-channel-list.playback .selected-channel-main b {
  min-width: 0;
  overflow: hidden;
  overflow-wrap: normal;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.selected-channel-list.playback .selected-channel-index,
.selected-channel-list.playback .selected-channel-status {
  overflow-wrap: normal;
  white-space: nowrap;
}

.selected-channel-list.playback .selected-channel-index {
  color: var(--text);
  font-size: 13px;
  text-align: right;
}

.selected-channel-list.playback .selected-channel-playback {
  grid-area: playback;
  display: flex;
  align-items: center;
  padding-left: 32px;
}

.selected-channel-list.playback .selected-channel-playback :deep(.el-date-editor) {
  flex: 0 1 330px;
  width: 330px;
  min-width: 280px;
}

.selected-channel-list.playback .selected-channel-actions {
  flex: 1 1 auto;
  flex-wrap: nowrap;
  gap: 6px;
}

.selected-channel-list.playback .selected-channel-actions :deep(.el-button + .el-button) {
  margin-left: 0;
}

.selected-channel-list.playback .selected-channel-remove {
  grid-area: remove;
}

.selected-channel-list:not(.playback) .selected-channel-main b {
  overflow: hidden;
  overflow-wrap: normal;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.multi-player-panel.is-multi-fullscreen {
  position: fixed;
  inset: 0;
  z-index: 3000;
  width: 100vw;
  height: 100vh;
  max-height: none;
  border-radius: 0;
  background: rgba(3, 10, 24, .98);
  box-shadow: 0 0 0 1px rgba(100, 203, 255, .22), 0 24px 70px rgba(0, 0, 0, .62);
}

.multi-player-panel :deep(.panel-inner) {
  padding: 10px;
}

.multi-player-panel.is-multi-fullscreen :deep(.panel-inner) {
  display: grid;
  grid-template-rows: minmax(0, 1fr);
  height: 100%;
  min-height: 0;
  padding: 0;
}

.multi-player-panel.is-multi-fullscreen .multi-player {
  grid-template-rows: minmax(0, 1fr) auto;
  min-height: 0;
  height: 100%;
}

.multi-player-panel.is-multi-fullscreen .multi-player :deep(.multi-grid) {
  grid-template-rows: auto minmax(0, 1fr);
  height: 100%;
  min-height: 0;
}

.multi-player-panel.is-multi-fullscreen .multi-player :deep(.grid-body) {
  aspect-ratio: auto;
  min-height: 0;
}

.multi-player {
  display: grid;
  grid-template-rows: auto auto;
  gap: 8px;
  min-height: 0;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
  padding: 8px;
}

.multi-player :deep(.multi-grid) {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  gap: 8px;
  height: auto;
  min-height: 0;
}

.multi-player :deep(.grid-toolbar) {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  min-width: 0;
  min-height: 32px;
  flex-wrap: nowrap;
}

.multi-player :deep(.grid-toolbar-summary) {
  min-width: 0;
}

.multi-player :deep(.grid-toolbar-actions) {
  display: flex;
  align-items: center;
  flex: 0 0 auto;
  gap: 6px;
}

.multi-player :deep(.grid-layout-title) {
  font-size: 13px;
  white-space: nowrap;
}

.multi-player :deep(.grid-toolbar button) {
  min-width: 40px;
  height: 30px;
  margin-left: 6px;
  border-radius: 5px;
}

.multi-player :deep(.grid-toolbar button.active) {
  border-color: rgba(103, 232, 249, .72);
  background: rgba(8, 145, 178, .36);
  color: #dffbff;
}

.multi-player :deep(.grid-body) {
  aspect-ratio: 16 / 9;
  min-height: 0;
  gap: 8px;
}

.multi-player :deep(.grid-cell) {
  position: relative;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border: 1px solid rgba(157, 185, 210, .18);
  border-radius: 8px;
  background: #05090f;
}

.multi-player :deep(.grid-cell.selected) {
  outline: 2px solid var(--cyan);
}

.multi-player :deep(.grid-cell-close) {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 5;
  width: 26px;
  height: 26px;
  min-width: 0;
  border-radius: 50%;
  background: rgba(3, 10, 24, .72);
  color: var(--text);
}

.multi-player :deep(.media-info-panel) {
  right: 6px;
  bottom: 48px;
  gap: 5px;
  width: min(280px, calc(100% - 12px));
  max-height: calc(100% - 58px);
  padding: 7px 8px;
}

.multi-player :deep(.ptz-panel) {
  position: absolute;
  top: 52px;
  right: 6px;
  z-index: 5;
  width: 132px;
  max-height: calc(100% - 108px);
  padding: 7px;
  overflow: auto;
  border: 1px solid rgba(100, 203, 255, .22);
  border-radius: 8px;
  background: rgba(3, 10, 24, .86);
  box-shadow: 0 14px 36px rgba(0, 0, 0, .32);
  opacity: .72;
}

.multi-player :deep(.ptz-grid) {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 4px;
}

.multi-player :deep(.ptz-grid button) {
  min-width: 0;
  padding: 0;
  aspect-ratio: 1;
  border-radius: 5px;
}

.multi-player :deep(.ptz-panel label) {
  display: grid;
  gap: 4px;
  margin: 8px 0;
  color: var(--muted);
  font-size: 12px;
}

.multi-player :deep(.lens-row) {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 4px;
  margin-top: 4px;
}

.multi-player :deep(.lens-row button) {
  min-width: 0;
  padding: 0 4px;
  font-size: 12px;
}

.multi-player :deep(.empty-cell) {
  display: grid;
  place-items: center;
  align-content: center;
  gap: 6px;
  width: 100%;
  height: 100%;
  min-height: 0;
  padding: 18px;
  color: var(--muted);
  text-align: center;
}

.multi-player :deep(.empty-cell b) {
  color: var(--text);
  overflow-wrap: anywhere;
}

.multi-player :deep(.empty-cell small) {
  color: var(--muted);
}

.multi-player :deep(.gmv-player) {
  position: relative;
  width: 100%;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: #02050a;
  color: var(--text);
}

.multi-player :deep(.gmv-video) {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
  background: #02050a;
}

.multi-pagination {
  display: flex;
  justify-content: center;
}

.image-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 12px;
}

.image-card {
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  color: inherit;
  background: rgba(3, 10, 24, .36);
  text-decoration: none;
}

.image-preview {
  display: grid;
  place-items: center;
  aspect-ratio: 4 / 3;
  background: rgba(2, 5, 10, .88);
  color: var(--muted);
}

.image-preview img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.image-meta {
  display: grid;
  gap: 4px;
  padding: 10px;
}

.image-meta b {
  overflow: hidden;
  color: var(--text);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-meta span {
  color: var(--muted);
  font-size: 12px;
}

.cover-large {
  display: block;
  width: 100%;
  max-height: 70vh;
  object-fit: contain;
  background: #02050a;
}

.device-detail {
  display: grid;
  gap: 10px;
}

.detail-row {
  display: grid;
  grid-template-columns: 1fr;
  gap: 10px;
}

.detail-row.two {
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
}

.detail-item {
  display: grid;
  grid-template-columns: 86px minmax(0, 1fr);
  gap: 10px;
  align-items: center;
  min-width: 0;
  padding: 12px 14px;
  border: 1px solid var(--component-border);
  border-radius: 8px;
  background: var(--component-bg-soft);
}

.detail-item.wide {
  grid-template-columns: 96px minmax(0, 1fr);
}

.detail-item span {
  min-width: 0;
  color: var(--muted);
  font-size: 12px;
  white-space: nowrap;
}

.detail-item b {
  display: flex;
  min-height: 22px;
  align-items: center;
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 14px;
  font-weight: 700;
}

:deep(.device-detail-drawer .el-drawer__body),
:deep(.camera-config-drawer .el-drawer__body),
:deep(.resource-capability-drawer .el-drawer__body) {
  padding: 18px 20px;
}

:deep(.device-detail-drawer .el-drawer__footer),
:deep(.camera-config-drawer .el-drawer__footer),
:deep(.resource-capability-drawer .el-drawer__footer) {
  padding: 12px 20px 18px;
  border-top: 1px solid var(--component-divider);
}

.resource-capability-content {
  display: grid;
  gap: 14px;
}

:deep(.resource-capability-drawer .el-table) {
  overflow: hidden;
  border: 1px solid var(--component-border);
  border-radius: 10px;
  background: var(--component-bg-soft) !important;
}

:deep(.camera-config-drawer .el-form-item) {
  margin-bottom: 18px;
}

:deep(.camera-config-drawer .el-form-item__label) {
  font-weight: 700;
}

.config-form :deep(.el-select),
.config-form :deep(.el-input-number) {
  width: 100%;
}

.drawer-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

@media (max-width: 1500px) {
  .channel-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

@media (max-width: 1100px) {
  .channel-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 900px) {
  .monitor-head {
    grid-template-columns: 1fr;
  }

  .monitor-actions {
    justify-content: flex-start;
  }

  .channel-grid,
  .image-grid {
    grid-template-columns: 1fr;
  }

  .channel-actions {
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }

  .detail-row.two {
    grid-template-columns: 1fr;
  }
}
</style>
