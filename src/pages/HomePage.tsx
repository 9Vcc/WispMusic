import { useEffect } from "react";
import useSWR from "swr";
import { Entrance } from "@/components/entrance";
import { HighQualitySection } from "@/components/home/high-quality-section";
import { NewSongSection } from "@/components/home/new-song-section";
import { RecentListenSection } from "@/components/home/recent-listen-section";
import { RecommendAndFMSection } from "@/components/home/recommend-and-fm-section";
import { Section, SUPPORTED_TYPES } from "@/components/home/section";
import { ToplistSection } from "@/components/home/toplist-section";
import { HomeSectionSkeleton } from "@/components/skeleton/home-skeleton";
import { useTitlebar } from "@/contexts/titlebar-context";
import {
	getHomepageData,
	getRecentListenListData,
} from "@/lib/services/homepage";
import { useUserStore } from "@/lib/store/userStore/userStore";
import type { HomeBlock, RecentListenListData } from "@/lib/types";

export default function Page() {
	const { setOnRefresh, setIsRefreshing } = useTitlebar();
	const { isLoggedin } = useUserStore();

	// 登录状态编进 key：状态变化时 SWR 自动用新 key 重新请求，
	// 无需手动 mutate（手动 mutate 会绕过去重，导致挂载时重复请求）
	const {
		data: homepageData,
		isLoading: isLoadingHomepage,
		mutate: mutateHomepage,
	} = useSWR<HomeBlock[]>(
		["homepage", isLoggedin],
		() => getHomepageData(true),
		{
			revalidateOnFocus: false,
		},
	);

	const {
		data: recentListenList,
		isLoading: isLoadingRecent,
		mutate: mutateRecent,
	} = useSWR<RecentListenListData | null>(
		["recentListen", isLoggedin],
		() => getRecentListenListData(),
		{
			revalidateOnFocus: false,
		},
	);

	// 注册刷新回调
	useEffect(() => {
		setOnRefresh(async () => {
			setIsRefreshing(true);
			try {
				await Promise.all([mutateHomepage(), mutateRecent()]);
			} finally {
				setIsRefreshing(false);
			}
		});
		return () => setOnRefresh(null);
	}, [setOnRefresh, setIsRefreshing, mutateHomepage, mutateRecent]);

	const supportedBlocks =
		homepageData?.filter((block) => SUPPORTED_TYPES.includes(block.showType)) ??
		[];

	// 渐进式渲染：推荐/FM 卡片不依赖任何请求，立即展示；
	// 其余区块各自挂骨架，谁的数据先到谁先入场
	return (
		<div className="flex min-h-full w-full flex-col gap-12 px-8 py-8">
			<Entrance>
				<RecommendAndFMSection />
			</Entrance>

			{/* 热门排行榜：数据自取，渐进渲染 */}
			<Entrance delay={0.06}>
				<ToplistSection />
			</Entrance>

			<Entrance delay={0.1}>
				<HighQualitySection />
			</Entrance>

			<Entrance delay={0.12}>
				<NewSongSection />
			</Entrance>

			{isLoadingRecent ? (
				<HomeSectionSkeleton />
			) : (
				recentListenList && (
					<Entrance delay={0.06}>
						<RecentListenSection resources={recentListenList.resources} />
					</Entrance>
				)
			)}

			{isLoadingHomepage ? (
				<>
					<HomeSectionSkeleton />
					<HomeSectionSkeleton />
				</>
			) : (
				supportedBlocks.map((block, index) => (
					<Entrance
						key={block.blockCode}
						delay={Math.min(0.12 + index * 0.06, 0.3)}
					>
						<Section block={block} />
					</Entrance>
				))
			)}
		</div>
	);
}
