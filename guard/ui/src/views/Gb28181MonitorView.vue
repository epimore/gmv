<template>
  <div v-if="!selectedDevice && monitorMode === 'devices'" class="page-grid viewport-card-page is-single-card-page"
    v-loading="loading">
    <GlassPanel class="span-12 fill-panel" title="监控信息" subtitle="按设备查看在线状态和注册时间">
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
      <el-table class="fill-table" :data="devices" height="100%" empty-text="暂无监控设备">
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
            range-separator="至" start-placeholder="默认开始时间" end-placeholder="默认结束时间" format="YYYY-MM-DD HH:mm:ss"
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
          <el-select v-model="mediaTransport" aria-label="媒体传输模式" style="width: 150px">
            <el-option label="UDP" value="udp" />
            <el-option label="TCP 主动" value="tcp_active" />
            <el-option label="TCP 被动" value="tcp_passive" />
          </el-select>
          <el-select v-if="multiMode === 'live'" v-model="multiDefaultStreamProfile" aria-label="多画面默认码流" style="width: 130px">
            <el-option label="主码流" value="main" />
            <el-option label="辅码流" value="sub" />
          </el-select>
          <el-tooltip :content="broadcastStatusText" placement="bottom">
            <el-button v-if="!broadcastSession" type="warning" :loading="broadcastStarting"
              :disabled="!canOperate || !selectedTreeChannels.length" @click="startMultiBroadcast">
              广播所选通道
            </el-button>
          </el-tooltip>
          <el-popover v-if="broadcastSession" placement="bottom-end" :width="520" trigger="click">
            <template #reference><el-button>目标状态</el-button></template>
            <div v-for="target in broadcastSession.summary.target_summaries" :key="target.leg_id" class="broadcast-target-row">
              <span>{{ target.device_id }} / {{ target.channel_id }}</span>
              <StatusPill :label="target.state" :tone="target.state === 'running' ? 'ONLINE' : target.state === 'failed' ? 'ERROR' : 'OFFLINE'" />
              <span>{{ target.profile || '-' }} · {{ target.transport }}</span>
              <el-button v-if="target.state === 'running'" link type="danger" @click="stopBroadcastLeg(target.leg_id)">停止</el-button>
            </div>
          </el-popover>
          <el-button v-if="broadcastSession" type="danger" :loading="broadcastStarting" @click="stopBroadcast">
            停止全部广播
          </el-button>
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
          <el-tree ref="multiDeviceTreeRef" class="device-channel-tree" :data="treeDeviceNodes" :props="treeProps"
            node-key="key" lazy :load="loadTreeNode" accordion :expand-on-click-node="true" :highlight-current="true">
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
              <el-icon>
                <QuestionFilled />
              </el-icon>
            </button>
          </el-tooltip>
        </div>
      </template>
      <div class="selected-channel-panel">
        <div class="selected-channel-list"
          :class="{ playback: multiMode === 'playback', empty: !selectedTreeChannels.length }">
          <article v-for="(channel, index) in selectedTreeChannels" :key="selectedChannelKey(channel)"
            class="selected-channel-item" :class="{ dragging: draggingTreeChannelIndex === index }" draggable="true"
            @dragstart="handleSelectedChannelDragStart(index)" @dragover.prevent
            @drop="handleSelectedChannelDrop(index)" @dragend="handleSelectedChannelDragEnd">
            <div class="selected-channel-main" @click="focusSelectedMultiChannel(channel)">
              <span v-if="multiMode === 'playback'" class="selected-channel-index">{{ index + 1 }}.</span>
              <el-tooltip :content="selectedChannelTooltip(channel)" placement="top">
                <b v-if="multiMode === 'playback'">{{ channel.device_id }} · {{ channel.channel_id }}</b>
                <b v-else>{{ index + 1 }}. {{ channel.device_id }} · {{ channel.channel_id }}</b>
              </el-tooltip>
              <span v-if="multiMode === 'playback'" class="selected-channel-status">{{
                multiPlaybackSelectionStatus(channel) }}</span>
              <el-select v-if="canBroadcastChannel(channel.channel)"
                v-model="broadcastTransportOverrides[selectedChannelKey(channel)]" clearable size="small"
                placeholder="继承广播传输" aria-label="目标广播传输覆盖" @click.stop>
                <el-option label="UDP" value="udp" />
                <el-option label="TCP 主动" value="tcp_active" />
                <el-option label="TCP 被动" value="tcp_passive" />
              </el-select>
            </div>
            <div v-if="multiMode === 'playback'" class="selected-channel-playback">
              <el-date-picker v-model="channel.playback_range" type="datetimerange" range-separator="至"
                start-placeholder="开始时间" end-placeholder="结束时间" :clearable="true" format="YYYY-MM-DD HH:mm:ss"
                :disabled="channel.playback_locked" size="small" />
              <div class="selected-channel-actions">
                <el-button size="small" :disabled="channel.playback_locked || !isValidPlaybackRange(multiDefaultRange)"
                  @click="restoreMultiPlaybackDefault(channel)">恢复默认</el-button>
                <el-button size="small" type="primary"
                  :disabled="channel.playback_locked || !isValidPlaybackRange(channel.playback_range)"
                  @click="confirmMultiPlayback(channel)">确认播放</el-button>
                <el-button v-if="channel.playback_locked && canReplayMultiPlayback(channel)" size="small" type="primary"
                  plain @click="replayMultiPlayback(channel)">重新播放</el-button>
                <el-button v-if="channel.playback_locked" size="small" type="warning" plain
                  @click="stopConfirmedMultiPlayback(channel)">停止并编辑</el-button>
              </div>
            </div>
            <el-button class="selected-channel-remove" type="danger" link
              @click.stop="removeTreeChannel(channel)">移除</el-button>
          </article>
          <el-empty v-if="!selectedTreeChannels.length" description="暂无已选通道" />
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-12 multi-player-panel" :class="{ 'is-multi-fullscreen': multiFullscreen }">
      <div class="multi-player">
        <GmvMultiGrid ref="multiGridRef" :grid-size="multiGridSize" :cells="multiGridCells"
          :visible-start="multiVisibleStart" @update:grid-size="handleMultiGridSizeChange"
          @snapshot="handleMultiSnapshot" @snapshot-error="handleMultiSnapshotError" @ptz="handleMultiPtz"
          @output-type-change="handleMultiOutputTypeChange" @playing="handleMultiPlaying"
          @playback-rate-change="handleMultiPlaybackRateChange" @playback-state-change="handleMultiPlaybackStateChange"
          @playback-seek="handleMultiPlaybackSeek" @playback-progress="handleMultiPlaybackProgress"
          @cloud-record-create="handleMultiCloudRecordCreate" @playback-error="handleMultiPlaybackError"
          @stream-profile-change="handleMultiStreamProfileChange"
          @network-degraded="handleMultiNetworkDegraded"
          @playback-switch-cancel="handleMultiPlaybackSwitchCancel" @close="handleMultiClose"
          @reorder="handleMultiReorder">
          <template #summary>
            <div class="multi-player-summary">
              <strong>多画面播放</strong>
              <span>{{ multiPlayerSubtitle }}</span>
            </div>
          </template>
          <template #actions>
            <div class="multi-player-actions">
              <template v-if="multiMode === 'playback'">
                <el-button :loading="multiBulkBusy" :disabled="multiPlaybackStarting || !multiControllableCells.length"
                  @click="toggleAllMultiPlayback">
                  {{ multiPauseActionLabel }}
                </el-button>
                <el-select :model-value="multiDesiredRate"
                  :disabled="multiBulkBusy || multiPlaybackStarting || !multiControllableCells.length" aria-label="统一倍速"
                  class="multi-rate-select" @change="setAllMultiPlaybackRate">
                  <el-option v-for="rate in playbackRates" :key="rate" :label="rate + 'x'" :value="rate" />
                </el-select>
              </template>
              <el-select v-else v-model="multiDefaultStreamProfile" aria-label="多画面默认码流" style="width: 130px">
                <el-option label="主码流" value="main" />
                <el-option label="辅码流" value="sub" />
              </el-select>
              <el-button plain @click="multiFullscreen = !multiFullscreen">{{ multiFullscreen ? '退出满屏' : '满屏'
                }}</el-button>
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

  <div v-else class="page-grid" :class="{ 'viewport-card-page has-summary-cards': showImages }">
    <GlassPanel class="span-12" title="通道监控" :subtitle="selectedDevice.device_id">
      <div class="monitor-head">
        <div class="device-summary">
          <StatusPill :label="selectedDevice.monitor_status === 1 ? '在线' : '离线'"
            :tone="selectedDevice.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
          <strong>{{ displayDeviceName(selectedDevice) }}</strong>
          <span>Session {{ selectedDevice.session_node_id || '-' }}</span>
        </div>
        <div class="monitor-actions">
          <el-select v-model="mediaTransport" aria-label="媒体传输模式" style="width: 150px">
            <el-option label="UDP" value="udp" />
            <el-option label="TCP 主动" value="tcp_active" />
            <el-option label="TCP 被动" value="tcp_passive" />
          </el-select>
          <el-select v-if="lastAction !== '历史回放'" v-model="selectedLiveProfile" aria-label="直播码流类型" style="width: 130px">
            <el-option label="主码流" value="main" />
            <el-option label="辅码流" value="sub" />
          </el-select>
          <el-button :loading="resourceLoading" @click="openResourceDrawer">资源能力</el-button>
          <el-tooltip :content="deviceBroadcastReasonText" placement="bottom"
            :disabled="selectedDevice.monitor_status === 1 && !!availableAudioOutputs.length">
            <el-button v-if="!broadcastSession" type="warning" :loading="broadcastStarting"
              :disabled="!canOperate || selectedDevice.monitor_status !== 1 || !availableAudioOutputs.length"
              @click="startBroadcast(selectedDevice.device_id)">设备广播</el-button>
          </el-tooltip>
          <el-button v-if="broadcastSession" type="danger" :loading="broadcastStarting"
            @click="stopBroadcast">停止广播</el-button>
          <el-button :loading="channelLoading" @click="reloadChannels">刷新通道</el-button>
          <el-button type="primary" @click="backToDevices">返回</el-button>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel v-if="showImages" class="span-12 fill-panel image-gallery-panel" title="抓拍图集"
      :subtitle="selectedChannelTitle">
      <div class="image-gallery-content" v-loading="imageLoading">
        <div class="image-gallery-toolbar">
          <div class="toolbar">
            <el-button @click="showImages = false">返回通道</el-button>
            <el-button :loading="imageLoading" @click="selectedChannel && loadImages(selectedChannel)">刷新图集</el-button>
          </div>
          <div class="image-time-filter">
            <el-date-picker v-model="imageStartTime" type="datetime" placeholder="开始时间" format="YYYY-MM-DD HH:mm:ss"
              :clearable="true" />
            <span>至</span>
            <el-date-picker v-model="imageEndTime" type="datetime" placeholder="结束时间" format="YYYY-MM-DD HH:mm:ss"
              :clearable="true" />
            <el-button type="primary" @click="queryImages">查询</el-button>
          </div>
        </div>
        <div v-if="images.length" class="image-grid">
          <article v-for="image in images" :key="image.session_node_id + ':' + image.image_id" class="image-card">
            <div class="image-preview">
              <el-image v-if="image.image_url" class="gallery-image" :src="image.image_url"
                :alt="image.file_name || image.image_id" :preview-src-list="imagePreviewUrls"
                :initial-index="imagePreviewUrls.indexOf(image.image_url)" fit="cover" lazy preview-teleported>
                <template #error><span>图片加载失败</span></template>
              </el-image>
              <span v-else>{{ image.can_preview ? '访问地址获取失败' : '不支持的图片格式' }}</span>
            </div>
            <div class="image-meta">
              <div><span>{{ formatTime(image.created_at_ms) }}</span></div>
              <el-button size="small" :type="selectedChannel?.over_pic_id === image.image_id ? 'success' : 'primary'"
                :disabled="!canOperate || selectedChannel?.over_pic_id === image.image_id"
                @click="setImageAsCover(image)">
                {{ selectedChannel?.over_pic_id === image.image_id ? '当前封面' : '设为封面' }}
              </el-button>
            </div>
          </article>
        </div>
        <el-empty v-else description="暂无抓拍图片" />
        <el-pagination class="image-pagination" background v-model:current-page="imagePage"
          v-model:page-size="imagePageSize" :page-sizes="[12, 24, 48]" :total="imageTotal"
          layout="total, sizes, prev, pager, next, jumper" @current-change="changeImagePage"
          @size-change="changeImagePageSize" />
      </div>
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
              <el-button-group class="channel-play-entry live">
                <el-dropdown class="channel-live-dropdown" trigger="click"
                  :disabled="!canPlayLive(channel) || playerRequesting"
                  @command="(value: LiveOutputType) => startLive(channel, value)">
                  <el-button class="channel-play-main" :disabled="!canPlayLive(channel) || playerRequesting"
                    :loading="isPlayRequesting('preview', channel)">直播</el-button>
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item v-for="option in liveOutputOptions" :key="option.value" :command="option.value">
                        {{ option.label }}
                      </el-dropdown-item>
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>
                <el-button class="channel-multi-tag" aria-label="加入多画面直播" :disabled="!canPlayLive(channel)"
                  @click="focusChannelInMultiView(channel, 'live')">·多</el-button>
              </el-button-group>
              <el-button-group class="channel-play-entry playback">
                <el-button class="channel-play-main" :disabled="!canPlayback(channel) || playerRequesting"
                  :loading="isPlayRequesting('playback', channel)" @click="requestPlayback(channel)">回放</el-button>
                <el-button class="channel-multi-tag" aria-label="加入多画面回放" :disabled="!canPlayback(channel)"
                  @click="focusChannelInMultiView(channel, 'playback')">·多</el-button>
              </el-button-group>
              <el-button @click="openCloudRecordings(channel)">下载</el-button>
              <el-button :disabled="!canSnapshot(channel)" :loading="deviceSnapshotLoading[channel.channel_id]"
                @click="requestDeviceSnapshot(channel)">抓拍</el-button>
              <el-button :disabled="!canViewImages(channel)" @click="openImages(channel)">图集</el-button>
              <el-dropdown class="channel-more-dropdown" trigger="click"
                @command="(command: string) => handleChannelMoreCommand(channel, command)">
                <el-button>更多</el-button>
                <template #dropdown>
                  <el-dropdown-menu>
                    <el-dropdown-item command="broadcast"
                      :disabled="!canOperate || !canBroadcastChannel(channel) || !!broadcastSession"
                      :title="channelBroadcastReason(channel)">
                      {{ broadcastStarting && broadcastScopeId === channel.channel_id ? '广播启动中' : '广播' }}
                    </el-dropdown-item>
                    <el-dropdown-item command="config" :disabled="!canOperate">配置</el-dropdown-item>
                  </el-dropdown-menu>
                </template>
              </el-dropdown>
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

    <el-dialog v-model="playbackRangeDialog" title="历史回放" width="980px">
      <div class="record-dialog-content">
        <section class="record-functional-block record-playback-panel">
          <h3>历史回放</h3>
          <div class="record-playback-controls">
            <el-date-picker v-model="playbackRange" type="datetimerange" range-separator="至" start-placeholder="回放开始时间"
              end-placeholder="回放结束时间" format="YYYY-MM-DD HH:mm:ss" :clearable="true" style="width: 100%" />
            <el-select :model-value="pendingPlaybackChannel ? channelPlaybackOutputType(pendingPlaybackChannel) : 'flv'"
              class="record-output-select" aria-label="回放播放格式"
              @change="(value: PlaybackOutputType) => pendingPlaybackChannel && setChannelPlaybackOutputType(pendingPlaybackChannel, value)">
              <el-option v-for="option in playbackOutputOptions" :key="option.value" :label="option.label"
                :value="option.value" />
            </el-select>
            <el-button type="primary" :disabled="!playbackRange" @click="confirmPlaybackRange">开始播放</el-button>
          </div>
        </section>
        <section class="record-functional-block device-record-panel">
          <div class="device-record-head">
            <h3>设备录像片段</h3>
            <el-tag v-if="recordState?.current_batch" size="small" effect="plain">
              最近更新：{{ formatTime(recordState.current_batch.created_at_ms) }}
            </el-tag>
            <el-tag v-else size="small" type="info" effect="plain">尚未更新</el-tag>
            <small v-if="recordState?.current_batch">
              更新范围：{{ formatRecordRange(recordState.current_batch.start_time_sec,
                recordState.current_batch.end_time_sec) }}
            </small>
          </div>
          <div class="record-update-controls">
            <el-button :type="recordRangeMode === 'week' ? 'primary' : 'default'"
              @click="selectRecordShortcut('week')">近一周</el-button>
            <el-button :type="recordRangeMode === 'month' ? 'primary' : 'default'"
              @click="selectRecordShortcut('month')">近一月</el-button>
            <el-date-picker v-model="recordUpdateRange" type="datetimerange" range-separator="至"
              start-placeholder="更新开始时间" end-placeholder="更新结束时间" format="YYYY-MM-DD HH:mm:ss" :clearable="true"
              @change="recordRangeMode = 'custom'" />
            <el-button type="primary" plain :loading="recordUpdating" :disabled="recordUpdateDisabled"
              @click="updateDeviceRecords">
              {{ recordRetryAfterSec > 0 ? `${recordRetryAfterSec}秒后可更新` : recordQuerying ? '更新中' : '更新' }}
            </el-button>
          </div>
          <el-alert v-if="recordQuerying" class="record-state-alert" type="info" :closable="false" show-icon
            title="设备录像正在更新，当前仍展示上一次完整结果" />
          <el-alert v-else-if="recordState?.attempt_batch?.status === 'FAILED'" class="record-state-alert" type="error"
            :closable="false" show-icon title="设备录像更新失败，可立即重试；上一次完整结果未受影响" />

          <section class="record-database-panel" v-loading="recordLoading">
            <div class="record-database-query">
              <span>数据库查询</span>
              <el-date-picker v-model="recordFilterStartTime" type="datetime" placeholder="查询开始时间"
                format="YYYY-MM-DD HH:mm:ss" :clearable="true" />
              <span>至</span>
              <el-date-picker v-model="recordFilterEndTime" type="datetime" placeholder="查询结束时间"
                format="YYYY-MM-DD HH:mm:ss" :clearable="true" />
              <el-button type="primary" @click="queryDeviceRecords">查询</el-button>
            </div>
            <el-table class="record-segment-table" :data="recordState?.segments || []" height="280"
              empty-text="数据库中暂无符合条件的录像片段" @row-click="selectRecordSegment">
              <el-table-column label="序号" width="72" align="center">
                <template #default="scope">{{ recordSequence(scope.$index) }}</template>
              </el-table-column>
              <el-table-column label="设备录像片段的时段" min-width="440">
                <template #default="scope">{{ formatRecordRange(scope.row.start_time_sec, scope.row.end_time_sec)
                  }}</template>
              </el-table-column>
              <el-table-column label="时长" width="120">
                <template #default="scope">{{ formatRecordDuration(scope.row.start_time_sec, scope.row.end_time_sec)
                  }}</template>
              </el-table-column>
            </el-table>
            <el-pagination v-if="recordTotal > recordPageSize" class="record-pagination" background
              layout="total, prev, pager, next" :total="recordTotal" :page-size="recordPageSize"
              :current-page="recordPage" @current-change="changeRecordPage" />
          </section>
        </section>
      </div>
    </el-dialog>

    <el-dialog v-model="playerDialog" :title="playerDialogTitle" width="960px" class="monitor-player-dialog"
      destroy-on-close @close="stopCurrentStream">
      <div v-if="selectedChannel" class="monitor-player">
        <div class="monitor-player-stage">
          <GmvPlayerView ref="singlePlayerRef" :sources="playerSources" :device-id="selectedChannel?.device_id"
            :channel-id="selectedChannel?.channel_id" :title="selectedChannelTitle" :status="playerStatus" :viewers="1"
            :media-mode="lastAction === '历史回放' ? 'playback' : 'live'" :stream-id="lastStream?.stream_id"
            :media-transport="mediaTransportLabel(singleMediaTransport)"
            :media-node-id="lastStream?.node_id" :session-node-id="lastStream?.session_node_id"
            :audio-codec="lastStream?.audio_codec" :poster="playerPoster" :capabilities="playerCapabilities"
            :controls="playerControls" :playback-duration-ms="playbackDurationMs"
            :playback-start-time-ms="playbackStartTimeMs" :playback-end-time-ms="playbackEndTimeMs"
            :cloud-record-locked-range="singleCloudRecordLockedRange" :output-type="singlePlayerOutputType"
            :output-options="singlePlayerOutputOptions" :output-switching="singleOutputSwitching"
            :stream-profile="lastAction === '历史回放' ? undefined : singleCommittedProfile"
            :stream-profile-verification="lastAction === '历史回放' ? undefined : singleCommittedProfileVerification"
            :stream-profile-options="streamProfileOptions" :stream-profile-switching="singleProfileSwitching"
            @output-type-change="handleSingleOutputTypeChange"
            @stream-profile-change="handleSingleStreamProfileChange"
            @network-degraded="handleSingleNetworkDegraded"
            :startup-text="singleMediaOperation ? singleStartupText : undefined"
            :startup-can-cancel="singleCheckpointReached" @snapshot="handleSingleSnapshot"
            @snapshot-error="handleSingleSnapshotError" @ptz="handlePlayerPtz" @playing="handleSinglePlaying"
            @playback-error="handleSinglePlaybackError" @cloud-record-request="openCloudRecordings(selectedChannel)"
            @cloud-record-create="handleSingleCloudRecordCreate"
            @playback-switch-cancel="handleSinglePlaybackSwitchCancel" @playback-rate-change="handlePlaybackRateChange"
            @playback-seek="handlePlaybackSeek" @playback-state-change="handlePlaybackStateChange"
            @playback-progress="handlePlaybackProgress" />
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

    <el-drawer v-model="cloudRecordingDrawer" title="设备录像下载" size="900px" class="cloud-recording-drawer">
      <template #header>
        <div class="cloud-recording-drawer-title">
          <h2>设备录像下载</h2>
          <span>下载任务</span>
        </div>
      </template>
      <div class="cloud-recording-content" v-loading="cloudRecordingLoading">
        <div class="cloud-recording-toolbar">
          <b>{{ cloudRecordingChannelTitle }}</b>
          <el-button :loading="cloudRecordingLoading" @click="loadCloudRecordings">刷新</el-button>
        </div>
        <p class="cloud-recording-description">将设备历史录像下载到平台，完成后可在线播放或下载到本地。</p>
        <div class="cloud-recording-create-form">
          <label>
            <span>开始时间</span>
            <el-date-picker v-model="cloudRecordingStartTime" type="datetime" format="YYYY-MM-DD HH:mm:ss"
              placeholder="请选择开始时间" :clearable="true" />
          </label>
          <label>
            <span>结束时间</span>
            <el-date-picker v-model="cloudRecordingEndTime" type="datetime" format="YYYY-MM-DD HH:mm:ss"
              placeholder="请选择结束时间" :clearable="true" />
          </label>
          <el-button :loading="cloudRecordingCreating" :disabled="!cloudRecordingStartTime || !cloudRecordingEndTime"
            type="primary" @click="createSelectedCloudRecording">开始下载</el-button>
        </div>
        <el-table :data="cloudRecordings" empty-text="暂无录像下载任务">
          <el-table-column label="下载时段" min-width="320">
            <template #default="{ row }">{{ formatRecordRange(row.start_time_sec, row.end_time_sec) }}</template>
          </el-table-column>
          <el-table-column label="状态" width="90">
            <template #default="{ row }"><el-tag :type="cloudStatusTag(row.status)">{{ cloudStatusText(row.status)
                }}</el-tag></template>
          </el-table-column>
          <el-table-column label="进度" width="115">
            <template #default="{ row }"><el-progress :percentage="row.progress_percent" :stroke-width="8" /></template>
          </el-table-column>
          <el-table-column label="文件大小" width="105" align="center" class-name="cloud-file-column"
            label-class-name="cloud-file-column">
            <template #default="{ row }">{{ formatBytes(row.final_size_bytes || row.current_size_bytes) }}</template>
          </el-table-column>
          <el-table-column label="操作" width="220" fixed="right" align="center" class-name="cloud-actions-column"
            label-class-name="cloud-actions-column">
            <template #default="{ row }">
              <div class="cloud-recording-actions">
                <el-button v-if="row.can_stop" type="warning" link :disabled="!canOperate"
                  @click="stopCloudTask(row)">停止下载</el-button>
                <el-button type="primary" link :disabled="!row.can_play" @click="playCloudTask(row)">播放</el-button>
                <el-button type="primary" link :disabled="!row.can_download"
                  @click="downloadCloudTask(row)">本地下载</el-button>
                <el-button type="danger" link :disabled="!canOperate || !row.can_delete"
                  @click="deleteCloudTask(row)">删除</el-button>
              </div>
            </template>
          </el-table-column>
        </el-table>
      </div>
    </el-drawer>

    <el-drawer v-model="configDrawer" title="相机业务配置" size="420px" class="camera-config-drawer" destroy-on-close>
      <el-form :model="configForm" label-width="110px" class="config-form">
        <el-form-item label="设备ID"><el-input v-model="configForm.device_id" disabled /></el-form-item>
        <el-form-item label="通道ID"><el-input v-model="configForm.channel_id" disabled /></el-form-item>
        <el-form-item label="名称"><el-input v-model="configForm.name" disabled /></el-form-item>
        <el-form-item label="别名"><el-input v-model="configForm.alias_name" maxlength="16" clearable /></el-form-item>
        <el-form-item label="排序"><el-input-number v-model="configForm.sort_no" :min="0" :max="999999" /></el-form-item>
        <el-form-item label="云台控制"><el-select v-model="configForm.ptz_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="语音广播"><el-select v-model="configForm.broadcast_enable"><el-option v-for="option in confOptions"
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

    <el-drawer v-model="resourceDrawer" title="资源识别覆盖管理" size="760px" class="resource-capability-drawer"
      destroy-on-close>
      <div v-loading="resourceLoading" class="resource-capability-content">
        <el-alert title="资源类型优先采用人工覆盖；没有有效覆盖时使用枚举、设备编码和 ParentID 自动识别。" type="info" :closable="false" />
        <el-table :data="resources" max-height="620" empty-text="暂无 Catalog 资源">
          <el-table-column prop="resource_id" label="资源 ID" min-width="190" show-overflow-tooltip />
          <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
          <el-table-column label="编码" width="72"><template #default="{ row }">{{ row.type_code || '-'
              }}</template></el-table-column>
          <el-table-column label="有效类型" width="120"><template #default="{ row }">{{ resourceKindText(row.effective_kind)
              }}</template></el-table-column>
          <el-table-column label="来源/状态" width="130"><template #default="{ row }">
              <el-tag :type="classificationTagType(row)">{{ classificationText(row) }}</el-tag>
            </template></el-table-column>
          <el-table-column label="业务所有者" min-width="170" show-overflow-tooltip><template #default="{ row }">{{
            row.effective_owner_scope }} · {{ row.effective_owner_id || '-' }}</template></el-table-column>
          <el-table-column label="操作" width="150" fixed="right"><template #default="{ row }">
              <el-button type="primary" link :disabled="!canManageResources" @click="editResource(row)">覆盖</el-button>
              <el-button type="warning" link
                :disabled="!canManageResources || !row.confirmation || row.confirmation.status !== 1"
                @click="resetResource(row)">恢复自动</el-button>
            </template></el-table-column>
        </el-table>
      </div>
    </el-drawer>

    <el-dialog v-model="resourceEditDialog" title="人工覆盖资源识别" width="520px" class="resource-confirm-dialog"
      destroy-on-close>
      <el-form :model="resourceForm" label-width="110px">
        <el-form-item label="资源 ID"><el-input :model-value="resourceEditing?.resource_id" disabled /></el-form-item>
        <el-form-item label="默认建议"><el-input
            :model-value="resourceKindText(resourceEditing?.suggested_kind || 'unknown')" disabled /></el-form-item>
        <el-form-item label="资源类型"><el-select v-model="resourceForm.resource_kind" style="width:100%">
            <el-option label="视频资源" value="video" /><el-option label="语音输入" value="audio_input" />
            <el-option label="语音输出" value="audio_output" /><el-option label="其它/否决" value="other" />
          </el-select></el-form-item>
        <el-form-item label="所有者范围"><el-radio-group v-model="resourceForm.owner_scope" @change="syncResourceOwner">
            <el-radio value="device">注册设备</el-radio><el-radio value="resource">Catalog 资源</el-radio>
          </el-radio-group></el-form-item>
        <el-form-item label="业务所有者"><el-select v-if="resourceForm.owner_scope === 'resource'"
            v-model="resourceForm.owner_id" filterable style="width:100%">
            <el-option v-for="channel in ownerResourceOptions" :key="channel.channel_id"
              :label="displayChannelName(channel) + ' · ' + channel.channel_id" :value="channel.channel_id" />
          </el-select><el-input v-else v-model="resourceForm.owner_id" disabled /></el-form-item>
        <el-form-item label="说明"><el-input v-model="resourceForm.remark" type="textarea" maxlength="255"
            show-word-limit /></el-form-item>
      </el-form>
      <template #footer><el-button @click="resourceEditDialog = false">取消</el-button><el-button type="primary"
          :loading="resourceSaving" @click="saveResource">保存人工覆盖</el-button></template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { onBeforeRouteLeave } from 'vue-router';
import { ElMessage, ElMessageBox } from 'element-plus';
import { QuestionFilled } from '@element-plus/icons-vue';
import {
  ApiError,
  errorMessage,
  cancelMediaOperation,
  closeStreamOutput,
  continueMediaOperation,
  createCloudRecording,
  createStreamOutput,
  deleteCloudRecording,
  getMediaTransport,
  getGbSessionNodeConfig,
  getGbChannelRecords,
  heartbeatGbPlaybackPresence,
  issueGbChannelImageAccess,
  issueCloudRecordingAccess,
  listGbChannelImages,
  listGbChannels,
  listGbDevicePage,
  listGbResources,
  listCloudRecordings,
  listNodes,
  queryGbChannelRecords,
  releaseStream,
  resetGbResourceConfirmation,
  saveGbResourceConfirmation,
  sendGbPtz,
  seekGbPlayback,
  setGbChannelCover,
  setGbPlaybackSpeed,
  setGbPlaybackState,
  startGbPlayback,
  startGbPreview,
  stopCloudRecording,
  stopGbBroadcastTarget,
  takeGbSnapshot,
  updateGbChannel,
  type GbChannelImageInfo,
  type GbChannelInfo,
  type GbChannelPayload,
  type GbChannelRecordsInfo,
  type GbBroadcastTargetPayload,
  type MediaTransport,
  type GbDeviceInfo,
  type GbPtzPayload,
  type GbRecordSegmentInfo,
  type GbResourceInfo,
  type GbSessionConfigInfo,
  type CloudRecordingStatus,
  type CloudRecordingSummary,
  type NodeInfo,
  type MediaOperationSummary,
  type PlaybackPresenceHeartbeatItem,
  type StreamSummary,
  type StreamOutputSummary,
  type StreamProfile,
} from '@/api/client';
import { startGbMicrophoneBroadcast, type GbBroadcastSession } from '@/audio/gbBroadcast';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { GmvMultiGrid, GmvPlayerView, type GmvCloudRecordRange, type GmvCodec, type GmvPlayerControlsConfig, type GmvPtzCommand, type GmvSource, type GmvStreamProfileOption, type GmvViewCapabilities } from 'gmv-player';
import { useAuthStore } from '@/stores/auth';
import { formatDateTime } from '@/utils/dateTime';

const auth = useAuthStore();
const singlePlayerRef = ref<InstanceType<typeof GmvPlayerView>>();
const multiGridRef = ref<InstanceType<typeof GmvMultiGrid>>();
type MultiDeviceTreeNode = { expanded: boolean; expand: (callback?: () => void) => void };
type MultiDeviceTreeInstance = {
  getNode: (key: string) => MultiDeviceTreeNode | undefined;
  setCurrentKey: (key: string) => void;
};
const multiDeviceTreeRef = ref<MultiDeviceTreeInstance>();
type LiveOutputType = 'flv' | 'hls' | 'll_hls' | 'fmp4';
type PlaybackOutputType = Exclude<LiveOutputType, 'll_hls'>;
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
const imagePreviewUrls = computed(() => images.value.map((image) => image.image_url).filter(Boolean));
const imageStartTime = ref<Date>();
const imageEndTime = ref<Date>();
const imagePage = ref(1);
const imagePageSize = ref(12);
const imageTotal = ref(0);
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
const selectedLiveProfile = ref<StreamProfile>('main');
const singleCommittedProfile = ref<StreamProfile>('main');
const singleCommittedProfileVerification = ref<'confirmed' | 'unverified' | 'unspecified'>('unspecified');
const singleProfileSwitching = ref(false);
let singleProfileGeneration = 0;
const singlePendingProfileSwitch = ref<{
  generation: number;
  source: StreamSummary;
  target: StreamSummary;
}>();
const networkSuggestionOpen = ref(false);
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
const cloudRecordingDrawer = ref(false);
const cloudRecordingLoading = ref(false);
const cloudRecordingCreating = ref(false);
const cloudRecordingChannel = ref<GbChannelInfo>();
const cloudRecordingSessionNodeId = ref('');
const cloudRecordingStartTime = ref<Date>();
const cloudRecordingEndTime = ref<Date>();
const cloudRecordings = ref<CloudRecordingSummary[]>([]);
const singleCloudRecordLockedRange = ref<GmvCloudRecordRange>();
const recordUpdateRange = ref<[Date, Date]>();
const recordRangeMode = ref<'week' | 'month' | 'custom'>('custom');
const recordFilterStartTime = ref<Date>();
const recordFilterEndTime = ref<Date>();
const recordPage = ref(1);
const recordPageSize = ref(10);
const recordState = ref<GbChannelRecordsInfo>();
const recordLoading = ref(false);
const recordUpdating = ref(false);
const recordNowMs = ref(Date.now());
let recordPollTimer: number | undefined;
let recordClockTimer: number | undefined;
let recordClockOffsetMs = 0;
let recordLoadGeneration = 0;
let lastFailedRecordBatchId = '';
const playbackGeneration = ref(0);
const singlePlaybackState = ref<'playing' | 'paused'>('playing');
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
const mediaTransport = ref<MediaTransport>('udp');
const singleMediaTransport = ref<MediaTransport>();
const broadcastTransportOverrides = reactive<Record<string, MediaTransport | ''>>({});
const deviceSnapshotLoading = reactive<Record<string, boolean>>({});
const channelOutputTypes = reactive<Record<string, LiveOutputType>>({});
const channelPlaybackOutputTypes = reactive<Record<string, PlaybackOutputType>>({});
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
const multiDefaultStreamProfile = ref<StreamProfile>('main');
const multiDefaultRange = ref<[Date, Date]>();
const multiPlaybackQueue = ref<string[]>([]);
const multiPlaybackStarting = ref(false);
const multiBulkBusy = ref(false);
const multiDesiredRate = ref(1);
const playbackRates = [0.5, 1, 2, 4];
const streamProfileOptions: GmvStreamProfileOption[] = [
  { value: 'main', label: '主码流' },
  { value: 'sub', label: '辅码流' },
];
const multiPlayVersions = reactive<Record<string, number>>({});
const viewerReleaseTasks = new Map<string, Promise<void>>();
const pendingViewerReleases = new Map<string, StreamSummary>();
const multiPreviewAborts = new Map<string, AbortController>();
const multiOutputAborts = new Map<string, AbortController>();
let stopCurrentStreamTask: Promise<boolean> | undefined;
let singlePreviewAbort: AbortController | undefined;
let singleProfileAbort: AbortController | undefined;
let singleOutputAbort: AbortController | undefined;
let playRequestSeq = 0;
let multiViewDisposed = false;
let playbackPresenceTimer: number | undefined;
let playbackPresenceInFlight = false;
const configForm = reactive<GbChannelPayload & { device_id?: string }>({ channel_id: '', device_id: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const canManageResources = computed(() => auth.session?.role === 'admin');
const cloudRecordingChannelTitle = computed(() => {
  const channel = cloudRecordingChannel.value;
  return channel ? `${displayChannelName(channel)} · ${channel.channel_id}` : '当前通道';
});
const recordQuerying = computed(() => recordState.value?.attempt_batch?.status === 'QUERYING');
const recordRetryAfterSec = computed(() => Math.max(0, Math.ceil(((recordState.value?.next_query_at_ms || 0) - recordNowMs.value) / 1000)));
const recordUpdateDisabled = computed(() => recordQuerying.value || recordUpdating.value || recordRetryAfterSec.value > 0);
const recordTotal = computed(() => recordState.value?.total || 0);
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
  media_transport?: MediaTransport;
  output_type: LiveOutputType;
  playback_start_sec?: number;
  playback_end_sec?: number;
  playback_position_sec?: number;
  playback_generation?: number;
  playback_rate?: number;
  playback_ack_rate?: number;
  playback_state?: 'playing' | 'paused';
  cloud_record_locked_range?: GmvCloudRecordRange;
  output?: StreamOutputSummary;
  output_switching?: boolean;
  stream_profile?: StreamProfile;
  profile_verification?: 'confirmed' | 'unverified' | 'unspecified';
  profile_switching?: boolean;
  profile_generation?: number;
  pending_profile_switch?: {
    generation: number;
    source: StreamSummary;
    target: StreamSummary;
  };
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
  output_type: LiveOutputType;
  playback_range?: [Date, Date];
  playback_locked?: boolean;
}
type TreeNodeData =
  | { key: string; label: string; kind: 'device'; device: GbDeviceInfo; leaf: false }
  | { key: string; label: string; kind: 'channel'; channel: GbChannelInfo; leaf: true };
const liveOutputOptions = [
  { value: 'flv', label: 'HTTP-FLV' },
  { value: 'hls', label: 'HLS-fMP4' },
  { value: 'll_hls', label: 'LL-HLS' },
  { value: 'fmp4', label: 'HTTP-fMP4' },
] satisfies Array<{ value: LiveOutputType; label: string }>;
const playbackOutputOptions = liveOutputOptions.filter(
  (option): option is { value: PlaybackOutputType; label: string } => option.value !== 'll_hls',
);

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
const broadcastStatusText = computed(() => {
  const summary = broadcastSession.value?.summary;
  if (!summary) return `将向 ${selectedTreeChannels.value.length} 个所选通道下发一份麦克风音频`;
  const running = summary.target_summaries.filter((target) => target.state === 'running').length;
  const failed = summary.target_summaries.filter((target) => target.state === 'failed').length;
  return `广播 ${summary.state}：运行 ${running}，失败 ${failed}，共 ${summary.target_summaries.length}`;
});
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
const singlePlayerOutputType = computed(() => {
  const channel = selectedChannel.value;
  if (!channel) return 'flv';
  return lastAction.value === '历史回放'
    ? channelPlaybackOutputType(channel)
    : channelOutputType(channel);
});
const singlePlayerOutputOptions = computed(() => lastAction.value === '历史回放' ? playbackOutputOptions : liveOutputOptions);
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

function pausedPlaybackPresenceItems(): PlaybackPresenceHeartbeatItem[] {
  const items: PlaybackPresenceHeartbeatItem[] = [];
  const single = lastStream.value;
  if (
    singlePlaybackState.value === 'paused'
    && single?.state === 'running'
    && single.playback_id
    && single.stream_id
    && single.subscription_id
  ) {
    items.push({
      playback_id: single.playback_id,
      stream_id: single.stream_id,
      subscription_id: single.subscription_id,
      generation: playbackGeneration.value,
    });
  }
  for (const cell of multiCells.value) {
    const stream = cell.stream;
    if (
      cell.mode === 'playback'
      && cell.playback_state === 'paused'
      && stream?.state === 'running'
      && stream.playback_id
      && stream.stream_id
      && stream.subscription_id
    ) {
      items.push({
        playback_id: stream.playback_id,
        stream_id: stream.stream_id,
        subscription_id: stream.subscription_id,
        generation: cell.playback_generation ?? 0,
      });
    }
  }
  return [...new Map(items.map((item) => [`${item.playback_id}:${item.stream_id}`, item])).values()];
}

const pausedPlaybackPresenceKey = computed(() => pausedPlaybackPresenceItems()
  .map((item) => `${item.playback_id}:${item.stream_id}:${item.subscription_id}:${item.generation}`)
  .sort()
  .join('|'));

function stopPlaybackPresenceHeartbeat() {
  if (playbackPresenceTimer !== undefined) {
    window.clearInterval(playbackPresenceTimer);
    playbackPresenceTimer = undefined;
  }
}

function syncPlaybackPresenceHeartbeat() {
  if (multiViewDisposed || !pausedPlaybackPresenceKey.value) {
    stopPlaybackPresenceHeartbeat();
    return;
  }
  if (playbackPresenceTimer === undefined) {
    playbackPresenceTimer = window.setInterval(() => {
      void heartbeatPausedPlaybacks();
    }, 60_000);
  }
}

async function heartbeatPausedPlaybacks() {
  const items = pausedPlaybackPresenceItems();
  if (multiViewDisposed || !items.length || playbackPresenceInFlight) return;
  playbackPresenceInFlight = true;
  try {
    const response = await heartbeatGbPlaybackPresence(items);
    let terminalCount = 0;
    for (const result of response.items) {
      if (!result.terminal) continue;
      const single = lastStream.value;
      if (
        single?.playback_id === result.playback_id
        && single.stream_id === result.stream_id
        && playbackGeneration.value === result.generation
      ) {
        lastStream.value = { ...single, endpoint: '', state: 'stopped' };
        singleOutput.value = undefined;
        singlePendingSwitch.value = undefined;
        singleOutputSwitching.value = false;
        singlePlaybackState.value = 'playing';
        terminalCount += 1;
      }
      const cell = multiCells.value.find((item) =>
        item.stream?.playback_id === result.playback_id
        && item.stream.stream_id === result.stream_id
        && (item.playback_generation ?? 0) === result.generation,
      );
      if (cell) {
        upsertMultiCell({
          ...cell,
          stream: undefined,
          sources: [],
          output: undefined,
          pending_switch: undefined,
          output_switching: false,
          playback_state: undefined,
          status: 'stopped',
          error: '暂停保活已超时，资源已释放',
        });
        terminalCount += 1;
      }
    }
    if (terminalCount) ElMessage.warning(`${terminalCount} 路暂停回放因保活超时已释放`);
  } catch {
    // 网络恢复后由下一次周期或 online/pageshow 事件补发；服务端超时负责最终回收。
  } finally {
    playbackPresenceInFlight = false;
    syncPlaybackPresenceHeartbeat();
  }
}

function handlePlaybackPresenceWakeup() {
  if (document.visibilityState === 'visible') void heartbeatPausedPlaybacks();
}

watch(pausedPlaybackPresenceKey, (key) => {
  syncPlaybackPresenceHeartbeat();
  if (key) void heartbeatPausedPlaybacks();
});
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
    streamSwitch: false,
    streamProfile: !!channel && lastAction.value !== '历史回放',
    aiOverlay: false,
  };
});
const playerControls = computed<GmvPlayerControlsConfig>(() => {
  const channel = selectedChannel.value;
  const playback = lastAction.value === '历史回放';
  const items: GmvPlayerControlsConfig['items'] = ['play', 'snapshot', 'fullscreen'];
  if (playback && channel && canPlayback(channel)) {
    items.splice(1, 0, 'playbackClip');
    items.push('timeline');
  }
  const overflowItems: GmvPlayerControlsConfig['items'] = [];
  overflowItems.push('outputType');
  overflowItems.push('info');
  if (!playback) overflowItems.push('streamProfile');
  if (channel && canAudio(channel)) overflowItems.push('audio');
  if (!playback && channel && canPtz(channel)) overflowItems.push('ptz');
  if (playback && channel && canPlayback(channel)) overflowItems.push('playbackRate');
  if (playback && channel) overflowItems.push('cloudRecord');
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
    mimeCodec: streamMimeCodec(lastStream.value, codec, hasAudio),
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
    mediaTransport: mediaTransportLabel(cell.media_transport),
    streamId: cell.stream?.stream_id,
    mediaNodeId: cell.stream?.node_id,
    sessionNodeId: cell.session_node_id,
    audioCodec: cell.stream?.audio_codec,
    poster: cell.poster,
    capabilities,
    controls: multiCellControls(capabilities),
    outputType: cell.mode === 'playback' ? playbackSafeOutputType(cell.output_type) : cell.output_type,
    outputOptions: cell.mode === 'playback' ? playbackOutputOptions : liveOutputOptions,
    outputSwitching: cell.output_switching,
    streamProfile: cell.mode === 'live' ? (cell.stream_profile || cell.stream?.effective_stream_profile || 'main') : undefined,
    streamProfileVerification: cell.mode === 'live' ? (cell.profile_verification || cell.stream?.stream_profile_verification || 'unspecified') : undefined,
    streamProfileOptions,
    streamProfileSwitching: cell.profile_switching,
    playbackDurationMs: playbackCellDurationMs(cell),
    playbackStartTimeMs: cell.mode === 'playback' && cell.playback_start_sec ? cell.playback_start_sec * 1_000 : undefined,
    playbackEndTimeMs: cell.mode === 'playback' && cell.playback_end_sec ? cell.playback_end_sec * 1_000 : undefined,
    cloudRecordLockedRange: cell.cloud_record_locked_range,
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
function channelPlaybackOutputType(channel: GbChannelInfo): PlaybackOutputType { return channelPlaybackOutputTypes[channelOutputKey(channel)] ?? 'flv'; }
function setChannelPlaybackOutputType(channel: GbChannelInfo, outputType: PlaybackOutputType) { channelPlaybackOutputTypes[channelOutputKey(channel)] = outputType; }
function playbackSafeOutputType(outputType: LiveOutputType): PlaybackOutputType { return outputType === 'll_hls' ? 'hls' : outputType; }
function setChannelOutputTypeForMode(channel: GbChannelInfo, mode: MultiMode, outputType: LiveOutputType) {
  if (mode === 'playback') setChannelPlaybackOutputType(channel, playbackSafeOutputType(outputType));
  else setChannelOutputType(channel, outputType);
}
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
    streamSwitch: cell.sources.length > 1,
    streamProfile: cell.mode === 'live',
    aiOverlay: false,
  };
}
function multiCellControls(capabilities: GmvViewCapabilities): GmvPlayerControlsConfig {
  const items: GmvPlayerControlsConfig['items'] = ['play'];
  const overflowItems: GmvPlayerControlsConfig['items'] = ['outputType', 'info'];
  if (capabilities.playback) {
    items.push('playbackClip');
    items.push('timeline');
    overflowItems.push('snapshot', 'fullscreen');
  } else {
    items.push('snapshot', 'fullscreen');
  }
  if (capabilities.audio) overflowItems.push('audio');
  if (capabilities.ptz) overflowItems.push('ptz');
  if (capabilities.streamSwitch) overflowItems.push('streamSwitch');
  if (capabilities.streamProfile) overflowItems.push('streamProfile');
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
  if (codec === 'h264' || codec === 'h.264' || codec === 'avc' || codec.startsWith('avc1')) return 'h264';
  if (codec === 'h265' || codec === 'h.265' || codec === 'hevc' || codec.startsWith('hev1') || codec.startsWith('hvc1')) return 'h265';
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
function streamMimeCodec(stream: StreamSummary | undefined, codec?: GmvCodec, hasAudio = false) {
  const actual = stream?.mime_codec?.trim();
  return actual || fmp4MimeCodec(codec, hasAudio);
}
function streamSourceLabel(codec: GmvCodec | undefined, hasAudio: boolean) {
  return `默认${hasAudio ? '音视频' : '静音'} · ${codec?.toUpperCase() || 'AUTO'}`;
}
function formatTime(value: number) {
  return formatDateTime(value);
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
function mediaTransportLabel(transport?: MediaTransport) {
  if (transport === 'tcp_active') return 'TCP 主动';
  if (transport === 'tcp_passive') return 'TCP 被动';
  return transport === 'udp' ? 'UDP' : undefined;
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
    mimeCodec: streamMimeCodec(stream, codec, hasAudio),
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
  for (const key of Object.keys(broadcastTransportOverrides)) delete broadcastTransportOverrides[key];
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
  if (!await stopAllMultiStreams({ quiet: true })) return;
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
  if (!await stopAllMultiStreams()) return;
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
  if (!await stopAllMultiStreams({ quiet: true })) return;
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
      output_type: 'flv',
      playback_range: multiMode.value === 'playback' ? defaultRange : undefined,
      playback_locked: false,
    });
    await startSelectedMultiChannel(selectedTreeChannelItems.value[selectedTreeChannelItems.value.length - 1]);
    return;
  }
  await stopMultiCell(key);
}
async function removeTreeChannel(channel: SelectedChannelRef) {
  const key = selectedChannelKey(channel);
  if (await stopMultiCell(key)) delete broadcastTransportOverrides[key];
}
function restoreMultiPlaybackDefault(channel: SelectedChannelRef) {
  if (channel.playback_locked) return;
  channel.playback_range = isValidPlaybackRange(multiDefaultRange.value)
    ? [new Date(multiDefaultRange.value[0]), new Date(multiDefaultRange.value[1])]
    : undefined;
}
function setSelectedMultiOutputType(key: string, outputType: LiveOutputType) {
  const selected = selectedTreeChannelItems.value.find((item) => selectedChannelKey(item) === key);
  if (selected) selected.output_type = outputType;
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
    output_type: multiMode.value === 'playback' ? playbackSafeOutputType(channel.output_type) : channel.output_type,
    stream_profile: multiMode.value === 'live' ? multiDefaultStreamProfile.value : 'main',
  });
  if (multiMode.value === 'live') {
    const cell = multiCells.value.find((item) => item.key === key);
    if (cell) await startMultiCell(cell);
  }
}
async function startMultiCell(cell: MultiViewCell) {
  const key = cell.key;
  const version = bumpMultiPlayVersion(key);
  const transport = mediaTransport.value;
  const controller = new AbortController();
  multiPreviewAborts.get(key)?.abort();
  multiPreviewAborts.set(key, controller);
  try {
    const requestId = `ui-multi-${cell.mode}-${Date.now()}-${cell.channel_id}`;
    const stream = cell.mode === 'live'
      ? await startGbPreview(cell.device_id, cell.channel_id, {
        request_id: requestId,
        session_node_id: cell.session_node_id,
        trans_mode: transport,
        output_type: cell.output_type,
        audio_codec: 'aac',
        stream_profile: cell.stream_profile || 'main',
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
        trans_mode: transport,
        output_type: playbackSafeOutputType(cell.output_type),
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
      stream_profile: stream.effective_stream_profile || cell.stream_profile || 'main',
      profile_verification: stream.stream_profile_verification || 'unspecified',
      media_transport: transport,
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
  return releaseViewerStream(stream);
}

async function releaseViewerStream(stream: StreamSummary) {
  if (!stream.subscription_id) return;
  const key = `${stream.stream_id}:${stream.subscription_id}`;
  pendingViewerReleases.set(key, stream);
  const existing = viewerReleaseTasks.get(key);
  if (existing) return existing;
  let task: Promise<void>;
  task = releaseStream(
    stream.stream_id,
    stream.subscription_id,
    `ui-stream-release-${crypto.randomUUID()}`,
  ).then(() => {
    pendingViewerReleases.delete(key);
  }).catch((error) => {
    if (error instanceof ApiError && error.retryable === false) {
      pendingViewerReleases.delete(key);
      return;
    }
    throw error;
  }).finally(() => {
    if (viewerReleaseTasks.get(key) === task) viewerReleaseTasks.delete(key);
  });
  viewerReleaseTasks.set(key, task);
  return task;
}

async function retryPendingViewerReleases() {
  const pending = [...pendingViewerReleases.values()];
  if (!pending.length) return true;
  const results = await Promise.allSettled(pending.map((stream) => releaseViewerStream(stream)));
  if (results.some((result) => result.status === 'rejected')) {
    ElMessage.error('仍有流资源释放失败，请重试');
    return false;
  }
  return true;
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
  const profileSource = cell.pending_profile_switch?.source;
  if (profileSource
    && (profileSource.stream_id !== cell.stream?.stream_id
      || profileSource.subscription_id !== cell.stream?.subscription_id)) {
    await stopMultiStream(profileSource);
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
  try {
    await disposeMultiCellMedia(cell);
  } catch (error) {
    ElMessage.error(errorMessage(error, '多画面资源释放失败，请重试'));
    return false;
  }
  multiCells.value = multiCells.value.filter((item) => item.key !== key);
  if (removeSelection) {
    selectedTreeChannelKeys.value = selectedTreeChannelKeys.value.filter((item) => item !== key);
    selectedTreeChannelItems.value = selectedTreeChannelItems.value.filter((item) => selectedChannelKey(item) !== key);
  }
  return true;
}
async function stopAllMultiStreams(options: { quiet?: boolean } = {}) {
  if (multiStopping.value) return false;
  const cells = [...multiCells.value];
  const streams = cells.map((cell) => cell.stream).filter((stream): stream is StreamSummary => !!stream?.stream_id);
  multiStopping.value = true;
  try {
    for (const cell of cells) bumpMultiPlayVersion(cell.key);
    for (const controller of multiPreviewAborts.values()) controller.abort();
    multiPreviewAborts.clear();
    for (const controller of multiOutputAborts.values()) controller.abort();
    multiOutputAborts.clear();
    multiPlaybackQueue.value = [];
    const releases = await Promise.allSettled(cells.map((cell) => disposeMultiCellMedia(cell)));
    if (releases.some((release) => release.status === 'rejected')) {
      ElMessage.error('多画面资源释放失败，请重试');
      return false;
    }
    multiCells.value = [];
    selectedTreeChannelKeys.value = [];
    selectedTreeChannelItems.value = [];
    for (const key of Object.keys(broadcastTransportOverrides)) delete broadcastTransportOverrides[key];
    multiGridManual.value = false;
    multiGridSize.value = 1;
    multiPage.value = 1;
    if (!options.quiet && streams.length) ElMessage.success('多画面已停止');
    return true;
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
    const pendingProfileSwitch = singlePendingProfileSwitch.value;
    if (cancelPending) {
      playRequestSeq += 1;
      singlePreviewAbort?.abort();
      singlePreviewAbort = undefined;
      singleOutputAbort?.abort();
      singleOutputAbort = undefined;
      singleProfileAbort?.abort();
      singleProfileAbort = undefined;
      singleProfileGeneration += 1;
      playerRequesting.value = false;
      pendingPlayKey.value = '';
    }
    const operation = singleMediaOperation.value;
    if (operation?.state === 'preparing') {
      await cancelMediaOperation(operation.operation_id).catch(() => undefined);
    }
    if (stream?.stream_id) {
      await closeTrackedOutputs(stream.stream_id, [
        output,
        pendingSwitch?.previous_output,
        pendingSwitch?.next_output,
      ]);
      try {
        await releaseViewerStream(stream);
        if (pendingProfileSwitch?.source
          && (pendingProfileSwitch.source.stream_id !== stream.stream_id
            || pendingProfileSwitch.source.subscription_id !== stream.subscription_id)) {
          await releaseViewerStream(pendingProfileSwitch.source);
        }
      } catch (error) {
        if (closeDialog) playerDialog.value = true;
        ElMessage.error(errorMessage(error, '流资源释放失败，请重试'));
        return false;
      }
    }
    if (closeDialog) playerDialog.value = false;
    lastStream.value = undefined;
    singleMediaTransport.value = undefined;
    singlePlaybackState.value = 'playing';
    singleOutput.value = undefined;
    singlePendingSwitch.value = undefined;
    singleOutputSwitching.value = false;
    singlePendingProfileSwitch.value = undefined;
    singleProfileSwitching.value = false;
    singleMediaOperation.value = undefined;
    singleWaitAcknowledged.value = false;
    if (clearAction) lastAction.value = '';
    return true;
  })().finally(() => {
    stopCurrentStreamTask = undefined;
  });
  return stopCurrentStreamTask;
}
async function focusChannelInMultiView(channel: GbChannelInfo, mode: MultiMode = 'live') {
  const device = selectedDevice.value;
  if (!device) return;
  if (!await stopCurrentStream()) return;
  if (!await retryPendingViewerReleases()) return;
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
async function handleSingleCloudRecordCreate(event: { startTimeMs: number; endTimeMs: number }) {
  const channel = selectedChannel.value;
  const sessionNodeId = lastStream.value?.session_node_id || selectedDevice.value?.session_node_id;
  if (!channel || !sessionNodeId) return;
  const range = normalizedCloudRecordRange(event);
  if (sameCloudRecordRange(singleCloudRecordLockedRange.value, range)) return;
  singleCloudRecordLockedRange.value = range;
  const created = await createQuickCloudRecording(channel, sessionNodeId, range);
  if (!created && sameCloudRecordRange(singleCloudRecordLockedRange.value, range)) {
    singleCloudRecordLockedRange.value = undefined;
  }
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
  multiGridRef.value?.confirmPlaybackProgress(event.index, (positionSec - cell.playback_start_sec) * 1_000);
  upsertMultiCell({ ...cell, playback_position_sec: positionSec });
}
async function handleMultiCloudRecordCreate(event: { index: number; payload: { startTimeMs: number; endTimeMs: number } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  const range = normalizedCloudRecordRange(event.payload);
  if (sameCloudRecordRange(cell.cloud_record_locked_range, range)) return;
  upsertMultiCell({ ...cell, cloud_record_locked_range: range });
  const created = await createQuickCloudRecording(cell.channel, cell.session_node_id, range);
  if (!created) {
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current && sameCloudRecordRange(current.cloud_record_locked_range, range)) {
      upsertMultiCell({ ...current, cloud_record_locked_range: undefined });
    }
  }
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
  return value === 'flv' || value === 'hls' || value === 'll_hls' || value === 'fmp4' ? value : undefined;
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
  if (cell.mode === 'playback' && outputType === 'll_hls') return;
  if (!cell.stream?.stream_id) {
    setChannelOutputTypeForMode(cell.channel, cell.mode, outputType);
    setSelectedMultiOutputType(cell.key, outputType);
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
    setChannelOutputTypeForMode(cell.channel, cell.mode, outputType);
    setSelectedMultiOutputType(cell.key, outputType);
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
    setChannelOutputTypeForMode(cell.channel, cell.mode, previousType);
    setSelectedMultiOutputType(cell.key, previousType);
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
  const profilePending = cell?.pending_profile_switch;
  if (cell && profilePending
    && cell.profile_generation === profilePending.generation
    && cell.stream?.stream_id === profilePending.target.stream_id
    && cell.stream?.subscription_id === profilePending.target.subscription_id) {
    upsertMultiCell({
      ...cell,
      stream_profile: profilePending.target.effective_stream_profile || profilePending.target.requested_stream_profile || cell.stream_profile || 'main',
      profile_verification: profilePending.target.stream_profile_verification || 'unspecified',
      pending_profile_switch: undefined,
      profile_switching: false,
      status: 'playing',
      error: undefined,
    });
    await releaseViewerStream(profilePending.source).catch(() => undefined);
  }
  const pending = cell?.pending_switch;
  if (!cell || !pending) return;
  upsertMultiCell({ ...cell, pending_switch: undefined, output_switching: false, status: 'playing', error: undefined });
  if (pending.previous_output && pending.previous_output.output_id !== pending.next_output.output_id) {
    await closeStreamOutput(cell.stream!.stream_id, pending.previous_output.output_id).catch(() => undefined);
  }
}

async function handleMultiStreamProfileChange(event: { index: number; payload: { profile: StreamProfile } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell || cell.mode !== 'live' || !cell.stream?.stream_id) return;
  if (cell.profile_switching || cell.output_switching || event.payload.profile === (cell.stream_profile || 'main')) return;
  const source = cell.stream;
  const generation = (cell.profile_generation || 0) + 1;
  upsertMultiCell({ ...cell, profile_generation: generation, profile_switching: true, error: undefined });
  try {
    const target = await startGbPreview(cell.device_id, cell.channel_id, {
      request_id: `ui-multi-profile-${Date.now()}-${cell.channel_id}-${event.payload.profile}`,
      session_node_id: cell.session_node_id,
      trans_mode: cell.media_transport || mediaTransport.value,
      output_type: cell.output_type,
      audio_codec: 'aac',
      stream_profile: event.payload.profile,
    });
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (!current || current.profile_generation !== generation
      || current.stream?.stream_id !== source.stream_id
      || current.stream?.subscription_id !== source.subscription_id) {
      await releaseViewerStream(target).catch(() => undefined);
      return;
    }
    upsertMultiCell({
      ...current,
      stream: target,
      sources: streamSources(target, 'live'),
      pending_profile_switch: { generation, source, target },
      profile_switching: true,
      status: 'reconnecting',
    });
  } catch (error) {
    const current = multiCells.value.find((item) => item.key === cell.key);
    if (current?.profile_generation === generation) {
      upsertMultiCell({ ...current, profile_switching: false, error: errorMessage(error, '切换主辅码流失败') });
    }
    if (!isAbortError(error)) ElMessage.error(errorMessage(error, `${cell.title} 切换主辅码流失败`));
  }
}

async function handleMultiNetworkDegraded(event: { index: number }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell || networkSuggestionOpen.value || (cell.stream_profile || 'main') !== 'main' || cell.profile_switching) return;
  networkSuggestionOpen.value = true;
  try {
    await ElMessageBox.confirm(
      `${cell.title} 网络持续不稳定，是否切换到辅码流以降低带宽占用？`,
      '网络质量提示',
      { type: 'warning', confirmButtonText: '切换到辅码流', cancelButtonText: '保持主码流' },
    );
    await handleMultiStreamProfileChange({ index: event.index, payload: { profile: 'sub' } });
  } catch {
    // 用户拒绝建议时保持当前码流。
  } finally {
    networkSuggestionOpen.value = false;
  }
}

async function handleMultiPlaybackError(event: { index: number; payload: { message: string } }) {
  const cell = multiCellAtVisibleIndex(event.index);
  const profilePending = cell?.pending_profile_switch;
  if (cell && profilePending) {
    upsertMultiCell({
      ...cell,
      stream: profilePending.source,
      sources: streamSources(profilePending.source, 'live'),
      pending_profile_switch: undefined,
      profile_switching: false,
      status: 'playing',
      error: undefined,
    });
    await releaseViewerStream(profilePending.target).catch(() => undefined);
    ElMessage.error(`切换主辅码流失败：${event.payload.message}`);
    return;
  }
  const pending = cell?.pending_switch;
  if (!cell) return;
  if (cell.mode === 'playback' && !pending) {
    await disposeMultiCellMedia(cell);
    upsertMultiCell({ ...cell, stream: undefined, sources: [], operation: undefined, status: 'error', error: event.payload.message });
    return;
  }
  if (!pending || !cell.stream) return;
  await closeStreamOutput(cell.stream.stream_id, pending.next_output.output_id).catch(() => undefined);
  setChannelOutputTypeForMode(cell.channel, cell.mode, pending.previous_type);
  setSelectedMultiOutputType(cell.key, pending.previous_type);
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
  if (cell.mode === 'playback' && !cell.output_switching && cell.operation?.state === 'preparing') {
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
  setChannelOutputTypeForMode(cell.channel, cell.mode, pending.previous_type);
  setSelectedMultiOutputType(cell.key, pending.previous_type);
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
  if (!await stopCurrentStream()) return;
  if (!await retryPendingViewerReleases()) return;
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
    const sessionNodeId = selectedDevice.value.session_node_id;
    const covers = await Promise.all(channelRows.map(async (channel) => {
      if (!channel.cover_image_id || !sessionNodeId) return channel;
      const access = await issueGbChannelImageAccess(
        channel.device_id,
        channel.channel_id,
        channel.cover_image_id,
        sessionNodeId,
      ).catch(() => undefined);
      return { ...channel, pic_url: access?.url || '' };
    }));
    channels.value = covers;
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
  if (!await stopBroadcast()) return;
  if (!await stopCurrentStream()) return;
  if (!await retryPendingViewerReleases()) return;
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
  if (lastAction.value === '历史回放' && outputType === 'll_hls') return;
  const previousType = lastAction.value === '历史回放'
    ? channelPlaybackOutputType(channel)
    : channelOutputType(channel);
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
    setChannelOutputTypeForMode(channel, lastAction.value === '历史回放' ? 'playback' : 'live', outputType);
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
  const profilePending = singlePendingProfileSwitch.value;
  const currentStream = lastStream.value;
  if (profilePending && currentStream
    && profilePending.generation === singleProfileGeneration
    && currentStream.stream_id === profilePending.target.stream_id
    && currentStream.subscription_id === profilePending.target.subscription_id) {
    singlePendingProfileSwitch.value = undefined;
    singleProfileSwitching.value = false;
    singleCommittedProfile.value = profilePending.target.effective_stream_profile || profilePending.target.requested_stream_profile || singleCommittedProfile.value;
    singleCommittedProfileVerification.value = profilePending.target.stream_profile_verification || 'unspecified';
    selectedLiveProfile.value = singleCommittedProfile.value;
    await releaseViewerStream(profilePending.source).catch(() => undefined);
  }
  const pending = singlePendingSwitch.value;
  const stream = lastStream.value;
  if (!pending || !stream) return;
  singlePendingSwitch.value = undefined;
  singleOutputSwitching.value = false;
  if (pending.previous_output && pending.previous_output.output_id !== pending.next_output.output_id) {
    await closeStreamOutput(stream.stream_id, pending.previous_output.output_id).catch(() => undefined);
  }
}

async function handleSingleStreamProfileChange(event: { profile: StreamProfile }) {
  const channel = selectedChannel.value;
  const source = lastStream.value;
  if (!channel || !source?.stream_id || lastAction.value === '历史回放') return;
  if (singleProfileSwitching.value || singleOutputSwitching.value || event.profile === singleCommittedProfile.value) return;
  const generation = ++singleProfileGeneration;
  const controller = new AbortController();
  singleProfileAbort?.abort();
  singleProfileAbort = controller;
  singleProfileSwitching.value = true;
  try {
    const target = await startGbPreview(channel.device_id, channel.channel_id, {
      request_id: `ui-single-profile-${Date.now()}-${event.profile}`,
      session_node_id: source.session_node_id || selectedDevice.value?.session_node_id,
      trans_mode: singleMediaTransport.value || mediaTransport.value,
      output_type: channelOutputType(channel),
      audio_codec: 'aac',
      stream_profile: event.profile,
    }, { signal: controller.signal });
    const current = lastStream.value;
    if (generation !== singleProfileGeneration || !current
      || current.stream_id !== source.stream_id
      || current.subscription_id !== source.subscription_id) {
      await releaseViewerStream(target).catch(() => undefined);
      return;
    }
    singlePendingProfileSwitch.value = { generation, source, target };
    lastStream.value = target;
  } catch (error) {
    if (generation === singleProfileGeneration) singleProfileSwitching.value = false;
    if (!isAbortError(error)) ElMessage.error(errorMessage(error, '切换主辅码流失败'));
  } finally {
    if (singleProfileAbort === controller) singleProfileAbort = undefined;
  }
}

async function handleSingleNetworkDegraded() {
  if (networkSuggestionOpen.value || singleCommittedProfile.value !== 'main' || singleProfileSwitching.value) return;
  networkSuggestionOpen.value = true;
  try {
    await ElMessageBox.confirm(
      '当前网络持续不稳定，是否切换到辅码流以降低带宽占用？',
      '网络质量提示',
      { type: 'warning', confirmButtonText: '切换到辅码流', cancelButtonText: '保持主码流' },
    );
    await handleSingleStreamProfileChange({ profile: 'sub' });
  } catch {
    // 用户拒绝建议时保持当前码流。
  } finally {
    networkSuggestionOpen.value = false;
  }
}

async function handleSinglePlaybackError(event: { message: string }) {
  const profilePending = singlePendingProfileSwitch.value;
  if (profilePending) {
    singlePendingProfileSwitch.value = undefined;
    singleProfileSwitching.value = false;
    lastStream.value = profilePending.source;
    await releaseViewerStream(profilePending.target).catch(() => undefined);
    ElMessage.error(`切换主辅码流失败：${event.message}`);
    return;
  }
  const pending = singlePendingSwitch.value;
  const stream = lastStream.value;
  const channel = selectedChannel.value;
  if (!pending || !stream || !channel) return;
  await closeStreamOutput(stream.stream_id, pending.next_output.output_id).catch(() => undefined);
  setChannelOutputTypeForMode(channel, lastAction.value === '历史回放' ? 'playback' : 'live', pending.previous_type);
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
  setChannelOutputTypeForMode(channel, lastAction.value === '历史回放' ? 'playback' : 'live', pending.previous_type);
  singleOutput.value = pending.previous_output;
  singlePendingSwitch.value = undefined;
  singleOutputSwitching.value = false;
  lastStream.value = { ...stream, endpoint: pending.previous_endpoint };
  ElMessage.info('已保持当前播放方式');
}

function requestPlayback(channel: GbChannelInfo) {
  stopRecordPolling();
  pendingPlaybackChannel.value = channel;
  playbackRange.value = undefined;
  recordUpdateRange.value = undefined;
  recordRangeMode.value = 'custom';
  recordFilterStartTime.value = undefined;
  recordFilterEndTime.value = undefined;
  recordPage.value = 1;
  recordState.value = undefined;
  recordLoading.value = false;
  recordUpdating.value = false;
  recordClockOffsetMs = 0;
  lastFailedRecordBatchId = '';
  playbackRangeDialog.value = true;
  startRecordClock();
  void loadDeviceRecords(channel);
}

async function startLive(channel: GbChannelInfo, outputType: LiveOutputType) {
  setChannelOutputType(channel, outputType);
  await startPlay('preview', channel);
}

function handleChannelMoreCommand(channel: GbChannelInfo, command: string) {
  if (command === 'broadcast') void startBroadcast(channel.channel_id);
  else if (command === 'config') openConfig(channel);
}

function selectRecordShortcut(mode: 'week' | 'month') {
  const end = new Date();
  const start = new Date(end);
  if (mode === 'week') {
    start.setTime(end.getTime() - 7 * 24 * 60 * 60 * 1000);
  } else {
    const targetMonth = end.getMonth() - 1;
    const day = end.getDate();
    start.setDate(1);
    start.setMonth(targetMonth);
    const lastDay = new Date(start.getFullYear(), start.getMonth() + 1, 0).getDate();
    start.setDate(Math.min(day, lastDay));
  }
  recordRangeMode.value = mode;
  recordUpdateRange.value = [start, end];
}

function validRecordRange(range?: [Date, Date]) {
  if (!range || range[0].getTime() >= range[1].getTime()) return false;
  return range[1].getTime() - range[0].getTime() <= 366 * 24 * 60 * 60 * 1000;
}

function formatRecordRange(startSec: number, endSec: number) {
  return `${formatDateTime(startSec * 1000)} 至 ${formatDateTime(endSec * 1000)}`;
}

function formatRecordDuration(startSec: number, endSec: number) {
  const durationSec = Math.max(0, endSec - startSec);
  const formatValue = (value: number) => String(Number(value.toFixed(2)));
  if (durationSec < 60 * 60) return `${formatValue(durationSec / 60)}分钟`;
  if (durationSec > 24 * 60 * 60) return `${formatValue(durationSec / 24 / 60 / 60)}天`;
  return `${formatValue(durationSec / 60 / 60)}小时`;
}

function recordSequence(index: number) {
  return (recordPage.value - 1) * recordPageSize.value + index + 1;
}

function selectRecordSegment(segment: GbRecordSegmentInfo) {
  playbackRange.value = [new Date(segment.start_time_sec * 1000), new Date(segment.end_time_sec * 1000)];
}

async function updateDeviceRecords() {
  const channel = pendingPlaybackChannel.value;
  const range = recordUpdateRange.value;
  const sessionNodeId = selectedDevice.value?.session_node_id || selectedListNodeId.value;
  if (!channel || !sessionNodeId) {
    ElMessage.error('当前设备缺少可用的 Session 节点');
    return;
  }
  if (!range || range[0].getTime() >= range[1].getTime()) {
    ElMessage.warning('请先选择录像检索时段');
    return;
  }
  if (!validRecordRange(range)) {
    ElMessage.warning('录像检索时间跨度不能超过366天');
    return;
  }
  recordLoadGeneration += 1;
  const generation = recordLoadGeneration;
  recordUpdating.value = true;
  try {
    const state = await queryGbChannelRecords(channel.device_id, channel.channel_id, {
      request_id: `ui-record-query-${Date.now()}`,
      session_node_id: sessionNodeId,
      start_time_sec: Math.floor(range[0].getTime() / 1000),
      end_time_sec: Math.floor(range[1].getTime() / 1000),
    });
    if (generation !== recordLoadGeneration || pendingPlaybackChannel.value?.device_id !== channel.device_id
      || pendingPlaybackChannel.value?.channel_id !== channel.channel_id) return;
    const current = recordState.value;
    applyRecordState({
      ...state,
      segments: current?.segments || [],
      total: current?.total || 0,
      page: current?.page || recordPage.value,
      page_size: current?.page_size || recordPageSize.value,
    }, true);
    ElMessage.success('设备录像更新任务已创建');
  } catch (error) {
    if (generation === recordLoadGeneration) {
      ElMessage.error(errorMessage(error, '设备录像更新失败'));
      void loadDeviceRecords(channel);
    }
  } finally {
    if (generation === recordLoadGeneration) recordUpdating.value = false;
  }
}

async function loadDeviceRecords(channel: GbChannelInfo, quiet = false) {
  const sessionNodeId = selectedDevice.value?.session_node_id || selectedListNodeId.value;
  if (!sessionNodeId) return;
  const generation = recordLoadGeneration;
  if (!quiet) recordLoading.value = true;
  try {
    const state = await getGbChannelRecords(channel.device_id, channel.channel_id, {
      session_node_id: sessionNodeId,
      start_time_sec: recordFilterStartTime.value ? Math.floor(recordFilterStartTime.value.getTime() / 1000) : undefined,
      end_time_sec: recordFilterEndTime.value ? Math.floor(recordFilterEndTime.value.getTime() / 1000) : undefined,
      page: recordPage.value,
      page_size: recordPageSize.value,
    });
    if (generation !== recordLoadGeneration || pendingPlaybackChannel.value?.device_id !== channel.device_id
      || pendingPlaybackChannel.value?.channel_id !== channel.channel_id) return;
    applyRecordState(state, quiet);
  } catch (error) {
    if (!quiet && generation === recordLoadGeneration) {
      ElMessage.error(errorMessage(error, '读取设备录像片段失败'));
    }
    if (quiet && generation === recordLoadGeneration) scheduleRecordPoll();
  } finally {
    if (!quiet && generation === recordLoadGeneration) recordLoading.value = false;
  }
}

function applyRecordState(state: GbChannelRecordsInfo, notifyFailure: boolean) {
  recordPage.value = state.page || recordPage.value;
  recordPageSize.value = state.page_size || recordPageSize.value;
  recordState.value = {
    ...state,
    total: state.total ?? state.segments.length,
    page: state.page || recordPage.value,
    page_size: state.page_size || recordPageSize.value,
  };
  recordClockOffsetMs = state.server_time_ms > 0 ? state.server_time_ms - Date.now() : 0;
  recordNowMs.value = Date.now() + recordClockOffsetMs;
  const failedBatchId = state.attempt_batch?.status === 'FAILED' ? state.attempt_batch.batch_id : '';
  if (notifyFailure && failedBatchId && failedBatchId !== lastFailedRecordBatchId) {
    ElMessage.error('设备录像更新失败，可立即重试；上一次结果已保留');
  }
  lastFailedRecordBatchId = failedBatchId;
  scheduleRecordPoll();
}

async function queryDeviceRecords() {
  const channel = pendingPlaybackChannel.value;
  if (!channel) return;
  const start = recordFilterStartTime.value?.getTime();
  const end = recordFilterEndTime.value?.getTime();
  if (start !== undefined && end !== undefined && start > end) {
    ElMessage.warning('数据库查询开始时间不能晚于结束时间');
    return;
  }
  recordPage.value = 1;
  await loadDeviceRecords(channel);
}

async function changeRecordPage(page: number) {
  const channel = pendingPlaybackChannel.value;
  if (!channel) return;
  recordPage.value = page;
  await loadDeviceRecords(channel);
}

function scheduleRecordPoll() {
  if (recordPollTimer !== undefined) window.clearTimeout(recordPollTimer);
  recordPollTimer = undefined;
  if (!playbackRangeDialog.value || !recordQuerying.value) return;
  recordPollTimer = window.setTimeout(async () => {
    const channel = pendingPlaybackChannel.value;
    if (channel) await loadDeviceRecords(channel, true);
  }, 2000);
}

function startRecordClock() {
  if (recordClockTimer !== undefined) window.clearInterval(recordClockTimer);
  recordNowMs.value = Date.now() + recordClockOffsetMs;
  recordClockTimer = window.setInterval(() => {
    recordNowMs.value = Date.now() + recordClockOffsetMs;
  }, 1000);
}

function stopRecordPolling() {
  recordLoadGeneration += 1;
  if (recordPollTimer !== undefined) window.clearTimeout(recordPollTimer);
  if (recordClockTimer !== undefined) window.clearInterval(recordClockTimer);
  recordPollTimer = undefined;
  recordClockTimer = undefined;
}

watch(playbackRangeDialog, (open) => {
  if (!open) stopRecordPolling();
});

watch(cloudRecordingDrawer, (open) => {
  if (open) void loadCloudRecordings();
  else cloudRecordingSessionNodeId.value = '';
});

function cloudSessionNodeId(): string {
  return cloudRecordingSessionNodeId.value || lastStream.value?.session_node_id || selectedDevice.value?.session_node_id || selectedListNodeId.value;
}

function openCloudRecordings(channel?: GbChannelInfo, sessionNodeId?: string) {
  const target = channel || selectedChannel.value || pendingPlaybackChannel.value;
  if (!target) {
    ElMessage.warning('请先选择通道');
    return;
  }
  const alreadyOpen = cloudRecordingDrawer.value;
  cloudRecordingChannel.value = target;
  cloudRecordingSessionNodeId.value = sessionNodeId
    || lastStream.value?.session_node_id
    || selectedDevice.value?.session_node_id
    || selectedListNodeId.value;
  cloudRecordingStartTime.value = undefined;
  cloudRecordingEndTime.value = undefined;
  if (playbackRangeDialog.value) playbackRangeDialog.value = false;
  cloudRecordingDrawer.value = true;
  if (alreadyOpen) void loadCloudRecordings();
}

async function loadCloudRecordings() {
  const channel = cloudRecordingChannel.value;
  const sessionNodeId = cloudSessionNodeId();
  if (!cloudRecordingDrawer.value || !channel || !sessionNodeId || cloudRecordingLoading.value) return;
  cloudRecordingLoading.value = true;
  try {
    const result = await listCloudRecordings(channel.device_id, channel.channel_id, sessionNodeId);
    cloudRecordings.value = result.items;
  } catch (error) {
    ElMessage.error(errorMessage(error, '下载列表加载失败'));
  } finally {
    cloudRecordingLoading.value = false;
  }
}

async function createSelectedCloudRecording() {
  const channel = cloudRecordingChannel.value;
  const startTime = cloudRecordingStartTime.value;
  const endTime = cloudRecordingEndTime.value;
  const sessionNodeId = cloudSessionNodeId();
  if (!channel || !startTime || !endTime || !sessionNodeId) {
    ElMessage.warning('请选择有效的录像时段');
    return;
  }
  const startTimeSec = Math.floor(startTime.getTime() / 1000);
  const endTimeSec = Math.floor(endTime.getTime() / 1000);
  if (startTimeSec >= endTimeSec || endTimeSec - startTimeSec > 7200) {
    ElMessage.warning('下载时段必须大于 0 且不超过 2 小时');
    return;
  }
  cloudRecordingCreating.value = true;
  try {
    await submitCloudRecording(channel, sessionNodeId, startTimeSec, endTimeSec);
    await loadCloudRecordings();
  } catch (error) {
    ElMessage.error(errorMessage(error, '下载任务创建失败'));
  } finally {
    cloudRecordingCreating.value = false;
  }
}

async function createQuickCloudRecording(
  channel: GbChannelInfo,
  sessionNodeId: string,
  range: { startTimeMs: number; endTimeMs: number },
): Promise<boolean> {
  const startTimeSec = Math.floor(Math.min(range.startTimeMs, range.endTimeMs) / 1_000);
  const endTimeSec = Math.floor(Math.max(range.startTimeMs, range.endTimeMs) / 1_000);
  if (endTimeSec - startTimeSec < 120) {
    ElMessage.warning('截取时长不能少于 2 分钟');
    return false;
  }
  if (endTimeSec - startTimeSec > 7200) {
    ElMessage.warning('截取时长不能超过 2 小时');
    return false;
  }
  try {
    await submitCloudRecording(channel, sessionNodeId, startTimeSec, endTimeSec);
    openCloudRecordings(channel, sessionNodeId);
    return true;
  } catch (error) {
    ElMessage.error(errorMessage(error, '下载任务创建失败'));
    return false;
  }
}

function normalizedCloudRecordRange(range: GmvCloudRecordRange): GmvCloudRecordRange {
  return {
    startTimeMs: Math.min(range.startTimeMs, range.endTimeMs),
    endTimeMs: Math.max(range.startTimeMs, range.endTimeMs),
  };
}

function sameCloudRecordRange(left: GmvCloudRecordRange | undefined, right: GmvCloudRecordRange) {
  return left?.startTimeMs === right.startTimeMs && left.endTimeMs === right.endTimeMs;
}

async function submitCloudRecording(channel: GbChannelInfo, sessionNodeId: string, startTimeSec: number, endTimeSec: number) {
  await createCloudRecording(channel.device_id, channel.channel_id, {
    request_id: `ui-cloud-recording-${Date.now()}-${channel.channel_id}`,
    session_node_id: sessionNodeId,
    start_time_sec: startTimeSec,
    end_time_sec: endTimeSec,
  });
  ElMessage.success('下载任务已创建');
}

async function stopCloudTask(task: CloudRecordingSummary) {
  try {
    await stopCloudRecording(task.task_id, `ui-cloud-recording-stop-${Date.now()}`);
    ElMessage.success('已提交停止请求，正在封装可播放文件');
    await loadCloudRecordings();
  } catch (error) {
    ElMessage.error(errorMessage(error, '停止下载失败'));
  }
}

async function deleteCloudTask(task: CloudRecordingSummary) {
  try {
    await ElMessageBox.confirm('将物理删除服务器上的录像文件，是否继续？', '删除下载', { type: 'warning' });
    await deleteCloudRecording(task.task_id, `ui-cloud-recording-delete-${Date.now()}`);
    ElMessage.success('下载文件已删除');
    await loadCloudRecordings();
  } catch (error) {
    if (error === 'cancel' || error === 'close') return;
    ElMessage.error(errorMessage(error, '删除下载失败'));
  }
}

async function playCloudTask(task: CloudRecordingSummary) {
  const tab = window.open('about:blank', '_blank');
  try {
    const access = await issueCloudRecordingAccess(task.task_id, 'inline');
    if (tab) tab.location.href = access.url;
    else window.open(access.url, '_blank');
  } catch (error) {
    tab?.close();
    ElMessage.error(errorMessage(error, '获取录像播放地址失败'));
  }
}

async function downloadCloudTask(task: CloudRecordingSummary) {
  try {
    const access = await issueCloudRecordingAccess(task.task_id, 'attachment');
    const link = document.createElement('a');
    link.href = access.url;
    link.download = access.file_name;
    link.rel = 'noopener';
    document.body.appendChild(link);
    link.click();
    link.remove();
  } catch (error) {
    ElMessage.error(errorMessage(error, '获取录像下载地址失败'));
  }
}

function cloudStatusText(status: CloudRecordingStatus): string {
  return ({ STARTING: '启动中', RUNNING: '下载中', STOPPING: '停止中', COMPLETED: '已完成', STOPPED: '已停止', PARTIAL: '部分完成', FAILED: '失败', DELETING: '删除中', DELETED: '已删除' })[status];
}

function cloudStatusTag(status: CloudRecordingStatus): 'success' | 'warning' | 'danger' | 'info' | 'primary' {
  if (status === 'COMPLETED') return 'success';
  if (status === 'FAILED') return 'danger';
  if (status === 'STOPPED' || status === 'PARTIAL') return 'warning';
  if (status === 'RUNNING' || status === 'STARTING') return 'primary';
  return 'info';
}

function formatBytes(bytes: number): string {
  if (!bytes) return '-';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
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
  singleCloudRecordLockedRange.value = undefined;
  lastAction.value = action;
  showImages.value = false;
  playerDialog.value = true;
  playerRequesting.value = true;
  singleMediaOperation.value = undefined;
  singleWaitAcknowledged.value = false;
  pendingPlayKey.value = playRequestKey(kind, channel);
  try {
    if (!await stopCurrentStream({ closeDialog: false, clearAction: false, cancelPending: false })) return;
    if (!await retryPendingViewerReleases()) return;
    const controller = new AbortController();
    singlePreviewAbort?.abort();
    singlePreviewAbort = controller;
    const transport = mediaTransport.value;
    const playbackRequestId = 'ui-monitor-playback-' + Date.now();
    const stream = kind === 'preview'
      ? await startGbPreview(
        channel.device_id,
        channel.channel_id,
        { request_id: 'ui-monitor-preview-' + Date.now(), session_node_id: selectedDevice.value?.session_node_id, trans_mode: transport, output_type: channelOutputType(channel), audio_codec: 'aac', stream_profile: selectedLiveProfile.value },
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
        { request_id: playbackRequestId, session_node_id: selectedDevice.value?.session_node_id, playback_id: playbackRequestId, start_time_sec: Math.floor(range![0].getTime() / 1000), end_time_sec: Math.floor(range![1].getTime() / 1000), trans_mode: transport, output_type: channelPlaybackOutputType(channel), audio_codec: 'aac' },
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
    singleCommittedProfile.value = stream.effective_stream_profile || selectedLiveProfile.value;
    singleCommittedProfileVerification.value = stream.stream_profile_verification || 'unspecified';
    singleMediaTransport.value = transport;
    if (kind === 'playback') {
      playbackGeneration.value = stream.playback_generation ?? 0;
      singlePlaybackState.value = 'playing';
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
    if (imageStartTime.value && imageEndTime.value && imageStartTime.value > imageEndTime.value) {
      ElMessage.warning('开始时间不能晚于结束时间');
      return;
    }
    const page = await listGbChannelImages(channel.device_id, channel.channel_id, {
      session_node_id: selectedDevice.value?.session_node_id || '',
      start_time_ms: imageStartTime.value?.getTime(),
      end_time_ms: imageEndTime.value?.getTime(),
      page: imagePage.value,
      page_size: imagePageSize.value,
    });
    imageTotal.value = page.total;
    imagePage.value = page.page;
    imagePageSize.value = page.page_size;
    const accessList = await Promise.all(page.items.map((image) => {
      if (!image.can_preview || !image.session_node_id) return Promise.resolve(undefined);
      return issueGbChannelImageAccess(
        image.device_id,
        image.channel_id,
        image.image_id,
        image.session_node_id,
      ).catch(() => undefined);
    }));
    images.value = page.items.map((image, index) => ({
      ...image,
      image_url: accessList[index]?.url || '',
    }));
    if (page.items.some((image) => image.can_preview) && !images.value.some((image) => image.image_url)) {
      ElMessage.warning('抓拍图片访问地址获取失败');
    }
  } catch (error) {
    images.value = [];
    imageTotal.value = 0;
    ElMessage.error(errorMessage(error, '抓拍图集加载失败'));
  } finally {
    imageLoading.value = false;
  }
}
async function openImages(channel: GbChannelInfo) {
  selectedChannel.value = channel;
  showImages.value = true;
  imagePage.value = 1;
  await loadImages(channel);
}
async function queryImages() {
  if (!selectedChannel.value) return;
  imagePage.value = 1;
  await loadImages(selectedChannel.value);
}
async function changeImagePage() {
  if (selectedChannel.value) await loadImages(selectedChannel.value);
}
async function changeImagePageSize() {
  imagePage.value = 1;
  if (selectedChannel.value) await loadImages(selectedChannel.value);
}
async function setImageAsCover(image: GbChannelImageInfo) {
  if (!selectedDevice.value || !selectedChannel.value) return;
  try {
    const updated = await setGbChannelCover(
      image.device_id,
      image.channel_id,
      image.image_id,
      selectedDevice.value.session_node_id,
    );
    const picUrl = image.image_url || selectedChannel.value.pic_url;
    const local = { ...updated, pic_url: picUrl };
    channels.value = channels.value.map((channel) => channel.channel_id === local.channel_id ? local : channel);
    selectedChannel.value = local;
    ElMessage.success('封面设置成功');
  } catch (error) {
    ElMessage.error(errorMessage(error, '封面设置失败'));
  }
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
    broadcast_enable: confValue(channel.broadcast_enable),
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
  await startBroadcastTargets([{
    device_id: selectedDevice.value.device_id,
    channel_id: scopeId,
    session_node_id: selectedDevice.value.session_node_id,
    trans_mode: mediaTransport.value,
  }], scopeId);
}
async function startMultiBroadcast() {
  const targets = selectedTreeChannels.value
    .filter((item) => canBroadcastChannel(item.channel))
    .map<GbBroadcastTargetPayload>((item) => ({
      device_id: item.device_id,
      channel_id: item.channel_id,
      session_node_id: item.session_node_id,
      trans_mode: broadcastTransportOverrides[selectedChannelKey(item)] || mediaTransport.value,
    }));
  if (!targets.length) {
    ElMessage.warning('请选择至少一个支持语音广播的在线通道');
    return;
  }
  await startBroadcastTargets(targets, `multi:${targets.length}`);
}
async function startBroadcastTargets(targets: GbBroadcastTargetPayload[], scopeId: string) {
  if (broadcastStarting.value || broadcastSession.value) return;
  broadcastStarting.value = true;
  broadcastScopeId.value = scopeId;
  try {
    const session = await startGbMicrophoneBroadcast(targets, mediaTransport.value);
    broadcastSession.value = session;
    void session.stopped.then(() => {
      if (broadcastSession.value === session) {
        broadcastSession.value = undefined;
        broadcastScopeId.value = '';
      }
    });
    ElMessage.success('语音广播已开始');
  } catch (error) {
    ElMessage.error(errorMessage(error, '语音广播启动失败'));
  } finally {
    broadcastStarting.value = false;
  }
}
async function stopBroadcastLeg(legId: string) {
  const session = broadcastSession.value;
  if (!session) return;
  try {
    session.summary = await stopGbBroadcastTarget(session.summary.broadcast_id, legId);
    ElMessage.success('广播目标已停止');
  } catch (error) {
    ElMessage.error(errorMessage(error, '广播目标停止失败'));
  }
}
async function stopBroadcast() {
  const session = broadcastSession.value;
  if (!session) return true;
  broadcastStarting.value = true;
  try {
    await session.stop();
    if (broadcastSession.value === session) broadcastSession.value = undefined;
    ElMessage.success('语音广播已停止');
    return true;
  } catch (error) {
    ElMessage.error(errorMessage(error, '语音广播停止失败，请重试'));
    return false;
  } finally {
    if (!broadcastSession.value) broadcastScopeId.value = '';
    broadcastStarting.value = false;
  }
}
async function saveConfig() {
  if (!selectedChannel.value) return;
  configSaving.value = true;
  try {
    const payload = { ...configForm, over_pic_id: selectedChannel.value.over_pic_id };
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
    singlePlaybackState.value = 'playing';
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
      singlePlaybackState.value = 'playing';
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
    singlePlaybackState.value = paused ? 'paused' : 'playing';
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
  singlePlaybackState.value = 'playing';
  try {
    await releaseViewerStream(stream);
  } catch (error) {
    ElMessage.error(errorMessage(error, '回放资源释放失败，请重试关闭播放器'));
    return;
  }
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
onMounted(() => {
  window.addEventListener('online', handlePlaybackPresenceWakeup);
  window.addEventListener('pageshow', handlePlaybackPresenceWakeup);
  document.addEventListener('visibilitychange', handlePlaybackPresenceWakeup);
});
onBeforeRouteLeave(async () => {
  if (!await stopBroadcast()) return false;
  if (!await stopAllMultiStreams({ quiet: true })) return false;
  if (!await stopCurrentStream()) return false;
  if (!await retryPendingViewerReleases()) return false;
  multiViewDisposed = true;
  stopPlaybackPresenceHeartbeat();
});
onBeforeUnmount(() => {
  multiViewDisposed = true;
  stopRecordPolling();
  stopPlaybackPresenceHeartbeat();
  window.removeEventListener('online', handlePlaybackPresenceWakeup);
  window.removeEventListener('pageshow', handlePlaybackPresenceWakeup);
  document.removeEventListener('visibilitychange', handlePlaybackPresenceWakeup);
  void stopBroadcast();
  void stopAllMultiStreams({ quiet: true });
  void stopCurrentStream();
  void retryPendingViewerReleases();
});
</script>

<style scoped>
.broadcast-target-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto auto;
  align-items: center;
  gap: 12px;
  padding: 8px 0;
}
.record-dialog-content {
  display: grid;
  gap: 16px;
}

.record-functional-block {
  display: grid;
  gap: 12px;
  padding: 18px;
  border: 1px solid var(--component-border);
  border-radius: 12px;
  background: var(--component-bg-soft);
}

.record-playback-controls {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 150px auto;
  gap: 10px;
}

.record-output-select {
  width: 150px;
}

.cloud-recording-content {
  display: grid;
  gap: 16px;
}

.cloud-recording-drawer-title {
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.cloud-recording-drawer-title h2 {
  margin: 0;
  color: var(--text);
  font-size: 18px;
}

.cloud-recording-drawer-title span {
  color: var(--muted);
  font-size: 13px;
}

.cloud-recording-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.cloud-recording-description {
  margin: 0;
  color: var(--muted);
  font-size: 13px;
}

.cloud-recording-create-form {
  display: grid;
  grid-template-columns: minmax(220px, 1fr) minmax(220px, 1fr) auto;
  align-items: end;
  gap: 12px;
}

.cloud-recording-create-form label {
  display: grid;
  gap: 4px;
}

.cloud-recording-create-form label>span {
  color: var(--muted);
  font-size: 12px;
}

.cloud-recording-create-form .el-date-editor {
  width: 100%;
}

.cloud-recording-actions {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
}

.cloud-recording-actions .el-button+.el-button {
  margin-left: 0;
}

:deep(.cloud-actions-column) {
  border-left: 1px solid var(--component-border);
}

@media (max-width: 760px) {
  .cloud-recording-create-form {
    grid-template-columns: 1fr;
  }
}

.record-functional-block h3 {
  margin: 0;
  color: var(--text);
  font-size: 16px;
}

.device-record-head {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}

.device-record-head small {
  min-width: 0;
  color: var(--muted);
  font-size: 12px;
  overflow-wrap: anywhere;
}

.record-update-controls {
  display: grid;
  grid-template-columns: auto auto minmax(430px, 1fr) auto;
  align-items: center;
  gap: 8px;
}

.record-update-controls .el-button+.el-button {
  margin-left: 0;
}

.record-state-alert {
  margin-top: 2px;
}

.record-database-panel {
  display: grid;
  gap: 12px;
  padding: 14px;
  border: 1px solid var(--component-border);
  border-radius: 10px;
  background: var(--component-bg-soft);
}

.record-database-query {
  display: grid;
  grid-template-columns: auto minmax(210px, 1fr) auto minmax(210px, 1fr) auto;
  align-items: center;
  gap: 8px;
  color: var(--muted);
  font-size: 13px;
}

.record-segment-table :deep(.el-table__row) {
  cursor: pointer;
}

.record-pagination {
  justify-self: end;
}

@media (max-width: 900px) {

  .record-update-controls,
  .record-database-query {
    grid-template-columns: 1fr;
  }

  .record-update-controls .el-date-editor,
  .record-database-query .el-date-editor {
    width: 100%;
  }
}

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

.multi-player-summary>strong {
  font-size: 16px;
}

.multi-player-summary>span {
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
  grid-column: span 2;
  width: 100%;
  min-width: 0;
}

.channel-play-entry .el-button {
  width: auto;
  height: 32px;
  padding: 7px 9px;
}

.channel-live-dropdown {
  flex: 1 1 auto;
  min-width: 0;
}

.channel-live-dropdown .channel-play-main {
  width: 100%;
}

.channel-more-dropdown,
.channel-more-dropdown .el-button {
  width: 100%;
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

.selected-channel-list.empty {
  grid-template-columns: minmax(0, 1fr);
  place-items: center;
  align-content: center;
  height: auto;
  min-height: 428px;
  overflow: hidden;
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

.multi-player :deep(.gmv-player.has-playback-timeline .media-info-panel) {
  bottom: 112px;
  max-height: calc(100% - 124px);
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
  align-content: start;
  gap: 12px;
}

.image-gallery-panel {
  overflow: hidden;
}

.image-gallery-content {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}

.image-gallery-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}

.image-time-filter {
  display: flex;
  align-items: center;
  gap: 8px;
}

.image-gallery-content>.image-grid {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding-right: 4px;
}

.image-gallery-content>.el-empty {
  flex: 1;
}

.image-pagination {
  flex: none;
  justify-content: flex-end;
  margin-top: 12px;
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

.gallery-image {
  width: 100%;
  height: 100%;
  cursor: zoom-in;
}

.image-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 4px;
  padding: 10px;
}

.image-meta>div {
  display: grid;
  min-width: 0;
  gap: 4px;
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
