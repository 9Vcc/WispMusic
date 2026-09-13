import { ErrorCircle24Regular } from "@fluentui/react-icons";
import { isRouteErrorResponse, useNavigate, useRouteError } from "react-router-dom";
import { WispButton } from "@/components/wisp-button";

export default function ErrorPage() {
	const error = useRouteError();
	const navigate = useNavigate();

	const isNotFound = isRouteErrorResponse(error) && error.status === 404;
	const title = isNotFound ? "页面不存在" : "页面出错了";
	const description = isNotFound
		? "你访问的页面不存在或已被移除"
		: "页面渲染时发生了意外错误，可以尝试返回首页或重新加载";

	const detail =
		error instanceof Error
			? `${error.name}: ${error.message}`
			: isRouteErrorResponse(error)
				? `${error.status} ${error.statusText}`
				: undefined;

	// bg-background：本页在「布局自身崩溃」时会顶替 RootLayout 渲染，
	// 而窗口底色只由 RootLayout 根节点绘制。不补这一层的话，纯色材质下
	// 崩溃页会整窗透明（文字浮在桌面上）。压在内容区上时该色与
	// --main-background 同色，不产生可见变化。
	return (
		<div className="flex flex-col items-center justify-center h-full min-h-[60vh] gap-4 select-none bg-background">
			<ErrorCircle24Regular className="size-12 text-foreground/40" />
			<div className="flex flex-col items-center gap-1">
				<span className="text-lg font-bold">{title}</span>
				<span className="text-sm text-foreground/60">{description}</span>
			</div>
			<div className="flex gap-2">
				<WispButton variant="default" size="default" onClick={() => navigate("/")}>
					返回首页
				</WispButton>
				<WispButton
					variant="default"
					size="default"
					onClick={() => window.location.reload()}
				>
					重新加载
				</WispButton>
			</div>
			{import.meta.env.DEV && detail && (
				<pre className="max-w-[80%] mt-4 p-3 rounded-lg bg-fill-subtle text-xs text-foreground/50 overflow-auto whitespace-pre-wrap">
					{detail}
				</pre>
			)}
		</div>
	);
}
